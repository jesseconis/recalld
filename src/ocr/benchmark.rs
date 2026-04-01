use anyhow::{Context, Result, bail};
use image::ImageReader;
use serde::{Deserialize, Serialize};
use tracing::trace;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::{OcrOptions, extract_text_with_options};

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkManifest {
    pub cases: Vec<BenchmarkCase>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkCase {
    pub name: String,
    pub image: PathBuf,
    pub expected_text: String,
    #[serde(default)]
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    pub manifest: String,
    pub variants: Vec<VariantReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VariantReport {
    pub label: String,
    pub max_width: Option<u32>,
    pub average_character_error_rate: f32,
    pub average_literal_recall: f32,
    pub average_literal_precision: f32,
    pub total_cases: usize,
    pub total_terms: usize,
    pub matched_terms: usize,
    pub cases: Vec<CaseReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseReport {
    pub name: String,
    pub image: String,
    pub expected_chars: usize,
    pub output_chars: usize,
    pub character_error_rate: f32,
    pub literal_recall: f32,
    pub literal_precision: f32,
    pub matched_terms: Vec<String>,
    pub missed_terms: Vec<String>,
    pub extra_terms: Vec<String>,
    pub output_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchmarkVariant {
    pub label: &'static str,
    pub options: OcrOptions,
}

impl BenchmarkVariant {
    pub fn default_runtime() -> Self {
        Self {
            label: "default",
            options: OcrOptions::default(),
        }
    }

    pub fn no_downscale() -> Self {
        Self {
            label: "no-downscale",
            options: OcrOptions { max_width: None },
        }
    }

    pub fn from_spec(spec: &str) -> Result<Self> {
        match spec {
            "default" => Ok(Self::default_runtime()),
            "no-downscale" => Ok(Self::no_downscale()),
            _ if spec.starts_with("max-width=") => {
                let raw = spec.trim_start_matches("max-width=");
                let width = raw
                    .parse::<u32>()
                    .with_context(|| format!("invalid max-width variant: {spec}"))?;
                Ok(Self {
                    label: Box::leak(format!("max-width={width}").into_boxed_str()),
                    options: OcrOptions::from_config_width(width),
                })
            }
            _ => bail!(
                "unsupported OCR variant '{spec}' (expected default, no-downscale, or max-width=<pixels>)"
            ),
        }
    }
}

pub fn load_manifest(path: &Path) -> Result<BenchmarkManifest> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read benchmark manifest: {}", path.display()))?;
    let manifest: BenchmarkManifest = toml::from_str(&text)
        .with_context(|| format!("failed to parse benchmark manifest: {}", path.display()))?;
    if manifest.cases.is_empty() {
        bail!("benchmark manifest must contain at least one [[cases]] entry");
    }
    Ok(manifest)
}

pub fn default_variants() -> Vec<BenchmarkVariant> {
    vec![BenchmarkVariant::default_runtime(), BenchmarkVariant::no_downscale()]
}

pub fn resolve_variants(specs: &[String]) -> Result<Vec<BenchmarkVariant>> {
    if specs.is_empty() {
        return Ok(default_variants());
    }

    specs.iter().map(|spec| BenchmarkVariant::from_spec(spec)).collect()
}

pub fn run_manifest(path: &Path, variants: &[BenchmarkVariant]) -> Result<BenchmarkReport> {
    let manifest = load_manifest(path)?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut variant_reports = Vec::with_capacity(variants.len());

    for variant in variants {
        let mut cases = Vec::with_capacity(manifest.cases.len());
        let mut total_cer = 0.0;
        let mut total_recall = 0.0;
        let mut total_precision = 0.0;
        let mut total_terms = 0usize;
        let mut matched_terms = 0usize;

        for case in &manifest.cases {
            let image_path = resolve_case_path(base_dir, &case.image);
            let image = ImageReader::open(&image_path)
                .with_context(|| format!("failed to open image: {}", image_path.display()))?
                .decode()
                .with_context(|| format!("failed to decode image: {}", image_path.display()))?;

            let output_text = extract_text_with_options(&image, variant.options)
                .with_context(|| format!("OCR failed for {}", image_path.display()))?;

            let case_report = score_case(case, &image_path, output_text);
            tracing::debug!("{:#?}", case_report);
            total_cer += case_report.character_error_rate;
            total_recall += case_report.literal_recall;
            total_precision += case_report.literal_precision;
            total_terms += case.terms.len();
            matched_terms += case_report.matched_terms.len();
            cases.push(case_report);
        }

        let total_cases = cases.len().max(1);
        variant_reports.push(VariantReport {
            label: variant.label.to_string(),
            max_width: variant.options.max_width,
            average_character_error_rate: total_cer / total_cases as f32,
            average_literal_recall: total_recall / total_cases as f32,
            average_literal_precision: total_precision / total_cases as f32,
            total_cases: cases.len(),
            total_terms,
            matched_terms,
            cases,
        });
    }

    Ok(BenchmarkReport {
        manifest: path.display().to_string(),
        variants: variant_reports,
    })
}

pub fn render_pretty(report: &BenchmarkReport) -> String {
    let mut out = String::new();
    let _ = writeln!(&mut out, "Manifest: {}", report.manifest);

    for variant in &report.variants {
        let max_width = variant
            .max_width
            .map(|width| width.to_string())
            .unwrap_or_else(|| "full-resolution".to_string());
        let _ = writeln!(&mut out);
        let _ = writeln!(&mut out, "Variant: {}", variant.label);
        let _ = writeln!(&mut out, "  max_width: {max_width}");
        let _ = writeln!(
            &mut out,
            "  avg_cer: {:.3}  avg_literal_recall: {:.3}  avg_literal_precision: {:.3}",
            variant.average_character_error_rate,
            variant.average_literal_recall,
            variant.average_literal_precision,
        );
        let _ = writeln!(
            &mut out,
            "  matched_terms: {}/{}  cases: {}",
            variant.matched_terms,
            variant.total_terms,
            variant.total_cases,
        );

        for case in &variant.cases {
            let _ = writeln!(
                &mut out,
                "  - {}  cer={:.3} recall={:.3} precision={:.3}",
                case.name,
                case.character_error_rate,
                case.literal_recall,
                case.literal_precision,
            );
            if !case.missed_terms.is_empty() {
                let _ = writeln!(&mut out, "    missed: {}", case.missed_terms.join(", "));
            }
        }
    }

    out
}

fn resolve_case_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn score_case(case: &BenchmarkCase, image_path: &Path, output_text: String) -> CaseReport {
    let expected = normalize_text_for_cer(&case.expected_text);
    let actual = normalize_text_for_cer(&output_text);
    let character_error_rate = if expected.is_empty() {
        if actual.is_empty() { 0.0 } else { 1.0 }
    } else {
        levenshtein_distance(&expected, &actual) as f32 / expected.chars().count() as f32
    };

    let output_compact = normalize_term_string(&output_text);
    let expected_terms = case
        .terms
        .iter()
        .map(|term| normalize_term_string(term))
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    let actual_terms = collect_terms(&output_text);

    let mut matched_terms = Vec::new();
    let mut missed_terms = Vec::new();
    for term in &case.terms {
        let normalized = normalize_term_string(term);
        if normalized.is_empty() {
            continue;
        }
        if matches_term(&normalized, &output_compact, &actual_terms) {
            matched_terms.push(term.clone());
        } else {
            missed_terms.push(term.clone());
        }
    }

    let mut extra_terms = Vec::new();
    for term in &actual_terms {
        if term.len() < 4 {
            continue;
        }
        if expected_terms
            .iter()
            .all(|expected_term| !matches_term(expected_term, term, &[]))
        {
            extra_terms.push(term.clone());
        }
    }
    extra_terms.sort();
    extra_terms.dedup();

    let literal_total = case.terms.len();
    let literal_recall = if literal_total == 0 {
        1.0
    } else {
        matched_terms.len() as f32 / literal_total as f32
    };
    let literal_precision = if matched_terms.is_empty() {
        if literal_total == 0 { 1.0 } else { 0.0 }
    } else {
        matched_terms.len() as f32 / (matched_terms.len() + extra_terms.len()) as f32
    };

    CaseReport {
        name: case.name.clone(),
        image: image_path.display().to_string(),
        expected_chars: case.expected_text.chars().count(),
        output_chars: output_text.chars().count(),
        character_error_rate,
        literal_recall,
        literal_precision,
        matched_terms,
        missed_terms,
        extra_terms,
        output_text,
    }
}

fn normalize_text_for_cer(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_term_string(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn collect_terms(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .map(normalize_term_string)
        .filter(|term| !term.is_empty())
        .collect()
}

fn matches_term(expected: &str, output_compact: &str, actual_terms: &[String]) -> bool {
    if output_compact.contains(expected) {
        return true;
    }

    actual_terms
        .iter()
        .map(|candidate| normalized_similarity(expected, candidate))
        .fold(0.0, f32::max)
        >= 0.82
}

fn normalized_similarity(left: &str, right: &str) -> f32 {
    let max_len = left.chars().count().max(right.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    1.0 - (levenshtein_distance(left, right) as f32 / max_len as f32)
}

fn levenshtein_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut prev = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut curr = vec![0usize; right_chars.len() + 1];

    for (left_idx, left_char) in left.chars().enumerate() {
        curr[0] = left_idx + 1;
        for (right_idx, right_char) in right_chars.iter().enumerate() {
            let cost = usize::from(left_char != *right_char);
            curr[right_idx + 1] = (curr[right_idx] + 1)
                .min(prev[right_idx + 1] + 1)
                .min(prev[right_idx] + cost);
        }
        prev.clone_from(&curr);
    }

    prev[right_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_variants_include_runtime_and_full_resolution() {
        let variants = default_variants();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].label, "default");
        assert_eq!(variants[1].label, "no-downscale");
    }

    #[test]
    fn variant_spec_parses_max_width() {
        let variant = BenchmarkVariant::from_spec("max-width=1600").unwrap();
        assert_eq!(variant.options.max_width, Some(1600));
    }

    #[test]
    fn substring_or_fuzzy_match_counts_literal_hit() {
        let actual_terms = vec!["gitnub".to_string(), "recalld".to_string()];
        assert!(matches_term("github", "gitnubrecalld", &actual_terms));
    }

    #[test]
    fn levenshtein_distance_handles_basic_cases() {
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
    }
}