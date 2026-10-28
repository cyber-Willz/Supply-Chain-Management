//! Supplier risk and performance scoring.
//!
//! Structural risk: logistic model fitted on the 24,112-supplier Supplier Risk Assessment dataset
//! (5-fold CV accuracy 0.92, AUC 0.97). Inputs are supplier attributes; missing inputs are imputed
//! with the dataset median and reported.
//! Performance: observed on-time / in-full / quality results. The datasets show the two are only
//! weakly related (labelled risk did not predict on-time delivery in the procurement data), so both
//! are reported side by side instead of being merged into one number.

pub struct RiskInputs {
    pub financial_stability: Option<f64>,
    pub geopolitical_risk: Option<f64>,
    pub alternatives: Option<f64>,
    pub environmental_compliance: Option<f64>,
    pub previous_disruptions: Option<f64>,
}

const INTERCEPT: f64 = 18.6731;
// (coefficient, dataset median used for imputation)
const FIN: (f64, f64) = (-0.28082, 56.6);
const GEO: (f64, f64) = (0.19887, 31.0);
const ALT: (f64, f64) = (-0.94107, 3.0);
const ENV: (f64, f64) = (-0.07604, 71.48);
const DIS: (f64, f64) = (1.43018, 1.0);

/// Returns (probability of high-risk flag, number of imputed inputs).
pub fn risk_probability(i: &RiskInputs) -> (f64, usize) {
    let mut missing = 0;
    let mut term = |v: Option<f64>, (w, med): (f64, f64)| {
        w * v.unwrap_or_else(|| {
            missing += 1;
            med
        })
    };
    let z = INTERCEPT
        + term(i.financial_stability, FIN)
        + term(i.geopolitical_risk, GEO)
        + term(i.alternatives, ALT)
        + term(i.environmental_compliance, ENV)
        + term(i.previous_disruptions, DIS);
    (1.0 / (1.0 + (-z).exp()), missing)
}

pub fn risk_band(p: f64) -> &'static str {
    if p < 0.30 {
        "low"
    } else if p < 0.70 {
        "medium"
    } else {
        "high"
    }
}

/// Weighted incident load per 100 received PO lines (minor 1, major 3, critical 5).
pub fn quality_score(minor: i64, major: i64, critical: i64, lines: i64) -> f64 {
    if lines <= 0 {
        return 100.0;
    }
    let weighted = (minor + 3 * major + 5 * critical) as f64 / lines as f64 * 100.0;
    100.0 * (1.0 - (weighted / 10.0).min(1.0))
}

/// 40% on-time, 30% in-full, 30% quality (all on a 0-100 scale).
pub fn performance_score(on_time: f64, in_full: f64, quality: f64) -> f64 {
    0.4 * on_time + 0.3 * in_full + 0.3 * quality
}

pub fn recommendation(perf: Option<f64>, band: &str) -> &'static str {
    match perf {
        Some(p) if p < 60.0 => "review",
        Some(p) if p < 75.0 => "monitor",
        _ if band == "high" => "monitor",
        _ => "ok",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_supplier_is_low_risk() {
        let (p, m) = risk_probability(&RiskInputs {
            financial_stability: Some(85.0),
            geopolitical_risk: Some(15.0),
            alternatives: Some(6.0),
            environmental_compliance: Some(90.0),
            previous_disruptions: Some(0.0),
        });
        assert_eq!(m, 0);
        assert!(p < 0.30, "{p}");
    }

    #[test]
    fn weak_supplier_is_high_risk_and_imputation_counted() {
        let (p, m) = risk_probability(&RiskInputs {
            financial_stability: Some(35.0),
            geopolitical_risk: Some(70.0),
            alternatives: None,
            environmental_compliance: None,
            previous_disruptions: Some(3.0),
        });
        assert_eq!(m, 2);
        assert!(p > 0.9, "{p}");
        assert_eq!(risk_band(p), "high");
    }

    #[test]
    fn no_inputs_means_median_profile() {
        let (p, m) = risk_probability(&RiskInputs {
            financial_stability: None,
            geopolitical_risk: None,
            alternatives: None,
            environmental_compliance: None,
            previous_disruptions: None,
        });
        assert_eq!(m, 5);
        assert!(p > 0.8); // callers must treat >=3 imputed inputs as "unknown", not "high"
    }

    #[test]
    fn scoring() {
        // aerospace profile: 43% on time, 89% in full, 368 incidents (243/91/34) over 29,666 PO lines
        let q = quality_score(243, 91, 34, 29_666);
        assert!((q - 76.9).abs() < 0.2, "{q}");
        let perf = performance_score(43.0, 89.0, q);
        assert!((perf - 67.0).abs() < 0.3, "{perf}");
        assert_eq!(recommendation(Some(perf), "low"), "monitor");
        assert_eq!(recommendation(Some(50.0), "low"), "review");
        assert_eq!(recommendation(None, "high"), "monitor");
    }
}
