use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoanPosition {
    pub owner: String,
    pub collateral_amount: u64,
    pub debt_amount: u64,
    pub collateral_ratio_bps: u64,
}
