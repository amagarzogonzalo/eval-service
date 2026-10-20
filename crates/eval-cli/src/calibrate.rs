//! `eval calibrate`: deterministic judge calibration over a JSONL
//! human-label set. The command never calls a model; it measures the
//! labels and scores a judge already produced, so the report is offline
//! and reproducible.

use anyhow::{Context, Result};
use eval_core::now_ms;
use eval_graders::calibration::{
    CalibrationItem, CalibrationParams, DatasetInfo, JudgeInfo, calibrate, dataset_hash,
    render_markdown,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub struct CalibrateArgs {
    pub dataset: PathBuf,
    pub out: PathBuf,
    pub markdown: Option<PathBuf>,
    pub judge_model: Option<String>,
    pub prompt_version: Option<String>,
    pub rubric: Option<String>,
    pub threshold: f64,
    pub kappa_min: f64,
    pub ci_level: f64,
    pub bootstrap_reps: u32,
    pub min_items: usize,
    pub min_position_items: usize,
    pub min_position_consistency: f64,
    pub max_verbosity_gap: f64,
    pub require_gating: bool,
    pub json: bool,
}

pub fn cmd_calibrate(args: CalibrateArgs) -> Result<i32> {
    let bytes = std::fs::read(&args.dataset)
        .with_context(|| format!("reading {}", args.dataset.display()))?;
    let text = String::from_utf8(bytes.clone()).context("calibration dataset is not UTF-8")?;
    let mut items = Vec::new();
    let mut ids = BTreeSet::new();
    for (line_no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let item: CalibrationItem = serde_json::from_str(line)
            .with_context(|| format!("{}:{}", args.dataset.display(), line_no + 1))?;
        if !ids.insert(item.id.clone()) {
            anyhow::bail!(
                "{}:{}: duplicate calibration item id `{}`",
                args.dataset.display(),
                line_no + 1,
                item.id
            );
        }
        items.push(item);
    }
    if items.is_empty() {
        anyhow::bail!("{} contains no calibration items", args.dataset.display());
    }

    let rubric = args.rubric.clone().or_else(|| {
        items
            .iter()
            .map(|item| item.rubric.trim())
            .find(|rubric| !rubric.is_empty())
            .map(str::to_string)
    });
    let judge_model = args.judge_model.clone().or_else(|| {
        let models: BTreeSet<&str> = items
            .iter()
            .filter_map(|item| item.judge_model.as_deref())
            .filter(|model| !model.is_empty())
            .collect();
        if models.len() == 1 {
            models.into_iter().next().map(str::to_string)
        } else {
            None
        }
    });
    if !(0.0..1.0).contains(&args.ci_level) {
        anyhow::bail!("--ci must be in (0, 1)");
    }
    if !(0.0..=10.0).contains(&args.threshold) {
        anyhow::bail!("--threshold must be in [0, 10]");
    }
    if !(-1.0..=1.0).contains(&args.kappa_min) {
        anyhow::bail!("--kappa-min must be in [-1, 1]");
    }
    if args.bootstrap_reps == 0 {
        anyhow::bail!("--bootstrap must be > 0");
    }
    if !(0.0..=1.0).contains(&args.min_position_consistency) {
        anyhow::bail!("--min-position-consistency must be in [0, 1]");
    }
    if !(0.0..=1.0).contains(&args.max_verbosity_gap) {
        anyhow::bail!("--max-verbosity-gap must be in [0, 1]");
    }
    let params = CalibrationParams {
        threshold: args.threshold,
        kappa_min: args.kappa_min,
        ci_level: args.ci_level,
        bootstrap_reps: args.bootstrap_reps,
        min_items: args.min_items,
        min_position_items: args.min_position_items,
        min_position_consistency: args.min_position_consistency,
        max_verbosity_gap: args.max_verbosity_gap,
    };
    let report = calibrate(
        &items,
        params,
        DatasetInfo {
            path: args.dataset.display().to_string(),
            blake3: dataset_hash(&bytes),
            n_items: items.len(),
        },
        JudgeInfo {
            model: judge_model.unwrap_or_else(|| "unknown".to_string()),
            prompt_version: args.prompt_version.clone(),
            rubric: rubric.unwrap_or_else(|| "unspecified".to_string()),
        },
        now_ms(),
    );

    if let Some(parent) = args.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    std::fs::write(&args.out, serde_json::to_string_pretty(&report)?)
        .with_context(|| format!("writing {}", args.out.display()))?;
    if let Some(markdown) = &args.markdown {
        std::fs::write(markdown, render_markdown(&report))
            .with_context(|| format!("writing {}", markdown.display()))?;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "calibration {} — {}",
            report.report_id,
            if report.decision.gating_eligible {
                "GATING ELIGIBLE"
            } else {
                "ADVISORY"
            }
        );
        if let Some(absolute) = &report.absolute {
            if let Some(kappa) = absolute.kappa {
                match &absolute.ci {
                    Some(ci) => println!(
                        "  kappa {kappa:.3}  {:.0}% CI [{:.3}, {:.3}]  n={}",
                        ci.level * 100.0,
                        ci.low,
                        ci.high,
                        absolute.n
                    ),
                    None => println!("  kappa {kappa:.3}  n={}", absolute.n),
                }
            }
        }
        for reason in &report.decision.reasons {
            println!("  advisory: {reason}");
        }
        println!("wrote {}", args.out.display());
        if let Some(markdown) = &args.markdown {
            println!("wrote {}", markdown.display());
        }
    }

    if args.require_gating && !report.decision.gating_eligible {
        return Ok(1);
    }
    Ok(0)
}
