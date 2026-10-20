"""Real local SMTP test inbox for the digest E2E notifier path.

The digest target's ``SMTPAdapter`` is exercised unchanged: it connects,
optionally negotiates STARTTLS, authenticates, and sends a MIME message
with the rendered PDF attached. This server implements enough of SMTP
(``EHLO``/``HELO``, ``STARTTLS``, ``AUTH LOGIN|PLAIN``, ``MAIL``,
``RCPT``, ``DATA``, ``RSET``, ``NOOP``, ``QUIT``) to be a genuine
endpoint. Every accepted message is persisted as ``.eml`` plus one
``eval.mail/v1`` index record, so the run can prove the side effect
landed instead of trusting a fake notifier.

It is a test destination, not a mail relay: it never forwards, binds
loopback by default, and requires AUTH when credentials are supplied.
A self-signed certificate is generated with ``openssl`` for STARTTLS;
``smtplib``'s default STARTTLS context does not verify it (the target
does not pass a context). An externally supplied certificate can be
used with ``--cert``/``--key``.

Run standalone:
    python smtp_sink.py --out DIR --port 2525 --user u --password p

The adapter starts one automatically for live cases when the notifier
is ``smtp`` and no external ``SMTP_HOST`` is configured.
"""

import argparse
import base64
import hashlib
import hmac
import json
import os
import secrets
import shutil
import signal
import socketserver
import ssl
import subprocess
import sys
import threading
import time
from email import policy
from email.parser import BytesParser

SCHEMA = "eval.mail/v1"
MAX_MESSAGE_BYTES = 8 * 1024 * 1024
_INDEX_LOCK = threading.Lock()


def log(message):
    sys.stderr.write(f"smtp-sink: {message}\n")
    sys.stderr.flush()


def ensure_certificate(cert_dir, cert_path="", key_path=""):
    cert_path = cert_path or os.path.join(cert_dir, "cert.pem")
    key_path = key_path or os.path.join(cert_dir, "key.pem")
    if os.path.isfile(cert_path) and os.path.isfile(key_path):
        return cert_path, key_path
    os.makedirs(cert_dir, exist_ok=True)
    openssl = os.environ.get("EVAL_SMTP_OPENSSL") or shutil.which("openssl")
    if not openssl:
        fallback = r"C:\Program Files\Git\usr\bin\openssl.exe"
        if os.path.isfile(fallback):
            openssl = fallback
    if not openssl:
        raise RuntimeError(
            "openssl not found; set EVAL_SMTP_OPENSSL or pass --cert/--key"
        )
    command = [
        openssl, "req", "-x509", "-newkey", "rsa:2048", "-sha256",
        "-nodes", "-days", "2", "-keyout", key_path, "-out", cert_path,
        "-subj", "/CN=localhost",
        "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
    ]
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise RuntimeError("openssl failed: {}".format((result.stderr or "")[-500:]))
    try:
        os.chmod(key_path, 0o600)
    except OSError:
        pass
    return cert_path, key_path


def append_index(path, record):
    line = (json.dumps(record, separators=(",", ":")) + "\n").encode("utf-8")
    with _INDEX_LOCK:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
        try:
            os.write(fd, line)
        finally:
            os.close(fd)


def describe_message(raw):
    message = BytesParser(policy=policy.default).parsebytes(raw)
    attachments = []
    has_pdf = False
    for part in message.walk():
        filename = part.get_filename()
        if filename:
            attachments.append(filename)
        if part.get_content_type() == "application/pdf" or filename and filename.lower().endswith(".pdf"):
            has_pdf = True
    return {
        "subject": str(message.get("Subject", "")),
        "from": str(message.get("From", "")),
        "to": str(message.get("To", "")),
        "message_id": str(message.get("Message-ID", "")),
        "has_pdf": has_pdf,
        "attachments": attachments,
    }


class SinkHandler(socketserver.StreamRequestHandler):
    """One SMTP connection. State is per connection."""

    def handle(self):
        self.connection.settimeout(120)
        self.tls = False
        self.helo = False
        self.authenticated = not bool(self.server.username)
        self.mail_from = ""
        self.rcpt_to = []
        self._old_files = []
        self._reply(220, "localhost ESMTP eval-digest-sink")
        while True:
            line = self.rfile.readline(65536)
            if not line:
                return
            text = line.decode("latin-1", "replace").rstrip("\r\n")
            if not text:
                continue
            command, _, argument = text.partition(" ")
            command = command.upper()
            keep = self._dispatch(command, argument.strip())
            if not keep:
                return

    def _dispatch(self, command, argument):
        if command in ("EHLO", "HELO"):
            self.helo = True
            if command == "EHLO":
                lines = ["localhost", f"SIZE {MAX_MESSAGE_BYTES}"]
                if not self.tls:
                    lines.append("STARTTLS")
                lines += ["AUTH PLAIN LOGIN", "OK"]
                self._reply_multi(250, lines)
            else:
                self._reply(250, "localhost")
            return True
        if command == "STARTTLS":
            if self.tls:
                self._reply(503, "already using TLS")
                return True
            self._reply(220, "ready to start TLS")
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(self.server.cert_path, self.server.key_path)
            self._old_files.append((self.rfile, self.wfile))
            self.connection = context.wrap_socket(self.connection, server_side=True)
            self.rfile = self.connection.makefile("rb")
            self.wfile = self.connection.makefile("wb")
            self.tls = True
            self.helo = False
            return True
        if command == "AUTH":
            return self._auth(argument)
        if command == "MAIL":
            if not self.authenticated:
                self._reply(530, "authentication required")
                return True
            self.mail_from = _address(argument)
            self.rcpt_to = []
            self._reply(250, "OK")
            return True
        if command == "RCPT":
            if not self.mail_from:
                self._reply(503, "MAIL FROM first")
                return True
            self.rcpt_to.append(_address(argument))
            self._reply(250, "OK")
            return True
        if command == "DATA":
            if not self.mail_from or not self.rcpt_to:
                self._reply(503, "MAIL FROM and RCPT TO first")
                return True
            self._reply(354, "end with <CRLF>.<CRLF>")
            raw = self._read_data()
            if raw is None:
                self._reply(552, "message too large")
                return True
            message_id = self._store(raw)
            self._reply(250, f"OK queued as {message_id}")
            self.mail_from = ""
            self.rcpt_to = []
            return True
        if command == "RSET":
            self.mail_from = ""
            self.rcpt_to = []
            self._reply(250, "OK")
            return True
        if command == "NOOP":
            self._reply(250, "OK")
            return True
        if command == "VRFY":
            self._reply(252, "cannot verify")
            return True
        if command == "QUIT":
            self._reply(221, "bye")
            return False
        self._reply(502, "command not implemented")
        return True

    def _auth(self, argument):
        parts = argument.split()
        mechanism = parts[0].upper() if parts else ""
        if mechanism == "PLAIN":
            blob = parts[1] if len(parts) > 1 else ""
            if not blob:
                self._reply(334, "")
                blob = self.rfile.readline(4096).decode("ascii", "replace").strip()
            if self._check_plain(blob):
                self._reply(235, "authentication successful")
            else:
                self._reply(535, "authentication failed")
            return True
        if mechanism == "LOGIN":
            username = parts[1] if len(parts) > 1 else ""
            if not username:
                self._reply(334, base64.b64encode(b"Username:").decode())
                username = self._read_b64()
            self._reply(334, base64.b64encode(b"Password:").decode())
            password = self._read_b64()
            if self._check(username, password):
                self._reply(235, "authentication successful")
            else:
                self._reply(535, "authentication failed")
            return True
        self._reply(504, "unsupported authentication mechanism")
        return True

    def _check_plain(self, blob):
        try:
            decoded = base64.b64decode(blob).decode("utf-8", "replace")
        except (ValueError, UnicodeError):
            return False
        parts = decoded.split("\0")
        if len(parts) != 3:
            return False
        return self._check(parts[1], parts[2])

    def _check(self, username, password):
        if not self.server.username:
            self.authenticated = True
            return True
        ok_user = hmac.compare_digest(username, self.server.username)
        ok_password = hmac.compare_digest(password, self.server.password)
        self.authenticated = ok_user and ok_password
        return self.authenticated

    def _read_b64(self):
        line = self.rfile.readline(4096).decode("ascii", "replace").strip()
        try:
            return base64.b64decode(line).decode("utf-8", "replace")
        except (ValueError, UnicodeError):
            return ""

    def _read_data(self):
        chunks = []
        size = 0
        while True:
            line = self.rfile.readline(65536)
            if not line:
                return None
            if line in (b".\r\n", b".\n"):
                break
            if line.startswith(b".."):
                line = line[1:]
            size += len(line)
            if size > MAX_MESSAGE_BYTES:
                return None
            chunks.append(line)
        return b"".join(chunks)

    def _store(self, raw):
        received_ms = int(time.time() * 1000)
        queue_id = f"{received_ms}-{secrets.token_hex(4)}"
        path = os.path.join(
            self.server.inbox_dir, "messages", f"msg-{queue_id}.eml"
        )
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(raw)
        details = describe_message(raw)
        record = {
            "schema": SCHEMA,
            "received_at_ms": received_ms,
            "queue_id": queue_id,
            "mail_from": self.mail_from,
            "rcpt_to": list(self.rcpt_to),
            "size": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "path": path,
            "tls": self.tls,
            "authenticated": self.authenticated,
        }
        record.update(details)
        append_index(self.server.index_path, record)
        log(f"accepted message {queue_id} size={len(raw)} pdf={details['has_pdf']}")
        return queue_id

    def _reply(self, code, text):
        try:
            self.wfile.write(f"{code} {text}\r\n".encode("latin-1"))
            self.wfile.flush()
        except OSError:
            pass

    def _reply_multi(self, code, lines):
        payload = ""
        for index, text in enumerate(lines):
            separator = " " if index == len(lines) - 1 else "-"
            payload += f"{code}{separator}{text}\r\n"
        try:
            self.wfile.write(payload.encode("latin-1"))
            self.wfile.flush()
        except OSError:
            pass


def _address(argument):
    if "<" in argument and ">" in argument:
        return argument[argument.index("<") + 1:argument.rindex(">")]
    return argument.split(":", 1)[-1].strip()


class SinkServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, address, handler, inbox_dir, cert_path, key_path,
                 username, password):
        super().__init__(address, handler)
        self.inbox_dir = inbox_dir
        self.index_path = os.path.join(inbox_dir, "inbox.jsonl")
        self.cert_path = cert_path
        self.key_path = key_path
        self.username = username
        self.password = password


def main():
    parser = argparse.ArgumentParser(description="local SMTP test inbox")
    parser.add_argument("--out", required=True, help="inbox directory")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--user", default="")
    parser.add_argument("--password", default="")
    parser.add_argument("--ready-file", default="")
    parser.add_argument("--cert-dir", default="")
    parser.add_argument("--cert", default="")
    parser.add_argument("--key", default="")
    args = parser.parse_args()

    inbox_dir = os.path.abspath(args.out)
    os.makedirs(os.path.join(inbox_dir, "messages"), exist_ok=True)
    cert_dir = args.cert_dir or os.path.join(inbox_dir, "certs")
    cert_path, key_path = ensure_certificate(cert_dir, args.cert, args.key)

    server = SinkServer(
        (args.host, args.port), SinkHandler, inbox_dir,
        cert_path, key_path, args.user, args.password,
    )
    host, port = server.server_address[:2]
    ready = {
        "schema": "eval.smtp-sink/v1",
        "host": host,
        "port": port,
        "inbox": inbox_dir,
        "index": os.path.join(inbox_dir, "inbox.jsonl"),
        "pid": os.getpid(),
        "started_at_ms": int(time.time() * 1000),
        "tls": True,
        "auth": bool(args.user),
    }
    if args.ready_file:
        ready_path = os.path.abspath(args.ready_file)
        os.makedirs(os.path.dirname(ready_path), exist_ok=True)
        temp = ready_path + ".tmp"
        with open(temp, "w", encoding="utf-8") as handle:
            json.dump(ready, handle)
        os.replace(temp, ready_path)
    log(f"listening on {host}:{port} inbox={inbox_dir}")

    def stop(signum, frame):
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        server.serve_forever(poll_interval=0.2)
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
