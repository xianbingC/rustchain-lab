use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// 一年按秒计，用于年化利率换算。
const SECONDS_PER_YEAR: u64 = 31_536_000;
/// 基点换算常量（100% = 10000 bps）。
const BPS_DENOMINATOR: u64 = 10_000;

/// 借贷仓位信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoanPosition {
    /// 仓位所属地址。
    pub owner: String,
    /// 抵押资产数量。
    pub collateral_amount: u64,
    /// 债务资产数量。
    pub debt_amount: u64,
    /// 当前抵押率（bps）。
    pub collateral_ratio_bps: u64,
}

/// 利率模型参数。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InterestRateModel {
    /// 基础借款年化利率（bps）。
    pub base_rate_bps: u64,
    /// 随资金利用率增加的利率斜率（bps）。
    pub utilization_slope_bps: u64,
}

impl Default for InterestRateModel {
    fn default() -> Self {
        Self {
            base_rate_bps: 500,
            utilization_slope_bps: 1_500,
        }
    }
}

/// 借贷池参数配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LendingConfig {
    /// 最低健康抵押率（bps），借款与提取抵押时强校验。
    pub min_collateral_ratio_bps: u64,
    /// 清算触发阈值（bps），低于该阈值允许清算。
    pub liquidation_ratio_bps: u64,
    /// 清算奖励（bps），按偿还债务比例给清算人额外抵押物。
    pub liquidation_bonus_bps: u64,
    /// 利率模型。
    pub interest_rate_model: InterestRateModel,
}

impl Default for LendingConfig {
    fn default() -> Self {
        Self {
            min_collateral_ratio_bps: 15_000,
            liquidation_ratio_bps: 12_000,
            liquidation_bonus_bps: 800,
            interest_rate_model: InterestRateModel::default(),
        }
    }
}

/// 清算执行结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiquidationOutcome {
    /// 被清算地址。
    pub borrower: String,
    /// 实际偿还债务数量。
    pub repaid_debt: u64,
    /// 实际扣除抵押品数量（含奖励）。
    pub seized_collateral: u64,
}

/// DeFi 借贷错误类型。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DefiError {
    /// 数量必须大于零。
    #[error("数量必须大于 0")]
    InvalidAmount,
    /// 时间回退会破坏计息过程。
    #[error("时间回退: now={now}, last={last}")]
    TimeRewind { now: i64, last: i64 },
    /// 仓位不存在。
    #[error("仓位不存在: owner={owner}")]
    PositionNotFound { owner: String },
    /// 抵押物不足。
    #[error("抵押物不足: available={available}, required={required}")]
    InsufficientCollateral { available: u64, required: u64 },
    /// 抵押率不足，拒绝操作。
    #[error("抵押率不足: actual={actual}bps, required={required}bps")]
    CollateralRatioTooLow { actual: u64, required: u64 },
    /// 当前仓位不可清算。
    #[error("仓位不可清算: ratio={ratio}bps, threshold={threshold}bps")]
    NotLiquidatable { ratio: u64, threshold: u64 },
    /// 算术溢出。
    #[error("算术溢出")]
    ArithmeticOverflow,
}

/// 借贷池状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LendingPool {
    /// 运行参数。
    pub config: LendingConfig,
    /// 仓位索引。
    pub positions: HashMap<String, LoanPosition>,
    /// 池内总抵押。
    pub total_collateral: u64,
    /// 池内总债务。
    pub total_debt: u64,
    /// 最近一次计息时间。
    pub last_accrual_ts: i64,
}

impl LendingPool {
    /// 创建借贷池。
    pub fn new(config: LendingConfig, now_ts: i64) -> Self {
        Self {
            config,
            positions: HashMap::new(),
            total_collateral: 0,
            total_debt: 0,
            last_accrual_ts: now_ts,
        }
    }

    /// 抵押资产入池。
    pub fn deposit_collateral(
        &mut self,
        owner: &str,
        amount: u64,
    ) -> Result<LoanPosition, DefiError> {
        if amount == 0 {
            return Err(DefiError::InvalidAmount);
        }

        let position = self
            .positions
            .entry(owner.to_string())
            .or_insert(LoanPosition {
                owner: owner.to_string(),
                collateral_amount: 0,
                debt_amount: 0,
                collateral_ratio_bps: u64::MAX,
            });

        position.collateral_amount = position
            .collateral_amount
            .checked_add(amount)
            .ok_or(DefiError::ArithmeticOverflow)?;
        self.total_collateral = self
            .total_collateral
            .checked_add(amount)
            .ok_or(DefiError::ArithmeticOverflow)?;
        position.collateral_ratio_bps =
            Self::calculate_collateral_ratio_bps(position.collateral_amount, position.debt_amount);
        Ok(position.clone())
    }

    /// 借出稳定币。
    pub fn borrow(
        &mut self,
        owner: &str,
        borrow_amount: u64,
        now_ts: i64,
    ) -> Result<LoanPosition, DefiError> {
        if borrow_amount == 0 {
            return Err(DefiError::InvalidAmount);
        }
        self.accrue_interest(now_ts)?;

        let position =
            self.positions
                .get_mut(owner)
                .ok_or_else(|| DefiError::PositionNotFound {
                    owner: owner.to_string(),
                })?;

        let next_debt = position
            .debt_amount
            .checked_add(borrow_amount)
            .ok_or(DefiError::ArithmeticOverflow)?;
        let ratio = Self::calculate_collateral_ratio_bps(position.collateral_amount, next_debt);
        if ratio < self.config.min_collateral_ratio_bps {
            return Err(DefiError::CollateralRatioTooLow {
                actual: ratio,
                required: self.config.min_collateral_ratio_bps,
            });
        }

        position.debt_amount = next_debt;
        position.collateral_ratio_bps = ratio;
        self.total_debt = self
            .total_debt
            .checked_add(borrow_amount)
            .ok_or(DefiError::ArithmeticOverflow)?;
        Ok(position.clone())
    }

    /// 偿还债务，返回实际偿还数量。
    pub fn repay(&mut self, owner: &str, repay_amount: u64, now_ts: i64) -> Result<u64, DefiError> {
        if repay_amount == 0 {
            return Err(DefiError::InvalidAmount);
        }
        self.accrue_interest(now_ts)?;

        let position =
            self.positions
                .get_mut(owner)
                .ok_or_else(|| DefiError::PositionNotFound {
                    owner: owner.to_string(),
                })?;

        let actual_repay = repay_amount.min(position.debt_amount);
        position.debt_amount = position
            .debt_amount
            .checked_sub(actual_repay)
            .ok_or(DefiError::ArithmeticOverflow)?;
        self.total_debt = self
            .total_debt
            .checked_sub(actual_repay)
            .ok_or(DefiError::ArithmeticOverflow)?;
        position.collateral_ratio_bps =
            Self::calculate_collateral_ratio_bps(position.collateral_amount, position.debt_amount);

        Ok(actual_repay)
    }

    /// 提取抵押资产。
    pub fn withdraw_collateral(
        &mut self,
        owner: &str,
        amount: u64,
        now_ts: i64,
    ) -> Result<LoanPosition, DefiError> {
        if amount == 0 {
            return Err(DefiError::InvalidAmount);
        }
        self.accrue_interest(now_ts)?;

        let position =
            self.positions
                .get_mut(owner)
                .ok_or_else(|| DefiError::PositionNotFound {
                    owner: owner.to_string(),
                })?;
        if position.collateral_amount < amount {
            return Err(DefiError::InsufficientCollateral {
                available: position.collateral_amount,
                required: amount,
            });
        }

        let next_collateral = position
            .collateral_amount
            .checked_sub(amount)
            .ok_or(DefiError::ArithmeticOverflow)?;
        let ratio = Self::calculate_collateral_ratio_bps(next_collateral, position.debt_amount);
        if position.debt_amount > 0 && ratio < self.config.min_collateral_ratio_bps {
            return Err(DefiError::CollateralRatioTooLow {
                actual: ratio,
                required: self.config.min_collateral_ratio_bps,
            });
        }

        position.collateral_amount = next_collateral;
        position.collateral_ratio_bps = ratio;
        self.total_collateral = self
            .total_collateral
            .checked_sub(amount)
            .ok_or(DefiError::ArithmeticOverflow)?;

        Ok(position.clone())
    }

    /// 对健康度不足的仓位执行清算。
    pub fn liquidate(
        &mut self,
        borrower: &str,
        repay_amount: u64,
        now_ts: i64,
    ) -> Result<LiquidationOutcome, DefiError> {
        if repay_amount == 0 {
            return Err(DefiError::InvalidAmount);
        }
        self.accrue_interest(now_ts)?;

        let position =
            self.positions
                .get_mut(borrower)
                .ok_or_else(|| DefiError::PositionNotFound {
                    owner: borrower.to_string(),
                })?;

        if position.debt_amount == 0 {
            return Err(DefiError::NotLiquidatable {
                ratio: u64::MAX,
                threshold: self.config.liquidation_ratio_bps,
            });
        }

        if position.collateral_ratio_bps >= self.config.liquidation_ratio_bps {
            return Err(DefiError::NotLiquidatable {
                ratio: position.collateral_ratio_bps,
                threshold: self.config.liquidation_ratio_bps,
            });
        }

        let actual_repay = repay_amount.min(position.debt_amount);
        let bonus = Self::mul_div_u64(
            actual_repay,
            self.config.liquidation_bonus_bps,
            BPS_DENOMINATOR,
        )?;
        let seized_total = position.collateral_amount.min(
            actual_repay
                .checked_add(bonus)
                .ok_or(DefiError::ArithmeticOverflow)?,
        );

        position.debt_amount = position
            .debt_amount
            .checked_sub(actual_repay)
            .ok_or(DefiError::ArithmeticOverflow)?;
        position.collateral_amount = position
            .collateral_amount
            .checked_sub(seized_total)
            .ok_or(DefiError::ArithmeticOverflow)?;
        position.collateral_ratio_bps =
            Self::calculate_collateral_ratio_bps(position.collateral_amount, position.debt_amount);
        self.total_debt = self
            .total_debt
            .checked_sub(actual_repay)
            .ok_or(DefiError::ArithmeticOverflow)?;
        self.total_collateral = self
            .total_collateral
            .checked_sub(seized_total)
            .ok_or(DefiError::ArithmeticOverflow)?;

        Ok(LiquidationOutcome {
            borrower: borrower.to_string(),
            repaid_debt: actual_repay,
            seized_collateral: seized_total,
        })
    }

    /// 当前借款年化利率（bps），使用简单利用率线性模型。
    pub fn current_borrow_rate_bps(&self) -> u64 {
        let utilization_bps = if self.total_collateral == 0 {
            0
        } else {
            ((self.total_debt as u128)
                .saturating_mul(BPS_DENOMINATOR as u128)
                .saturating_div(self.total_collateral as u128)) as u64
        };

        self.config
            .interest_rate_model
            .base_rate_bps
            .saturating_add(
                (utilization_bps as u128)
                    .saturating_mul(self.config.interest_rate_model.utilization_slope_bps as u128)
                    .saturating_div(BPS_DENOMINATOR as u128) as u64,
            )
    }

    /// 执行全池计息，返回新增利息总额。
    pub fn accrue_interest(&mut self, now_ts: i64) -> Result<u64, DefiError> {
        if now_ts < self.last_accrual_ts {
            return Err(DefiError::TimeRewind {
                now: now_ts,
                last: self.last_accrual_ts,
            });
        }
        if now_ts == self.last_accrual_ts || self.total_debt == 0 {
            self.last_accrual_ts = now_ts;
            return Ok(0);
        }

        let delta_secs = (now_ts - self.last_accrual_ts) as u64;
        let annual_rate_bps = self.current_borrow_rate_bps();
        let mut total_interest = 0u64;

        for position in self.positions.values_mut() {
            if position.debt_amount == 0 {
                continue;
            }

            let interest =
                Self::mul_div_u64(position.debt_amount, annual_rate_bps, BPS_DENOMINATOR)?
                    .checked_mul(delta_secs)
                    .ok_or(DefiError::ArithmeticOverflow)?
                    .checked_div(SECONDS_PER_YEAR)
                    .ok_or(DefiError::ArithmeticOverflow)?;
            if interest == 0 {
                continue;
            }

            position.debt_amount = position
                .debt_amount
                .checked_add(interest)
                .ok_or(DefiError::ArithmeticOverflow)?;
            position.collateral_ratio_bps = Self::calculate_collateral_ratio_bps(
                position.collateral_amount,
                position.debt_amount,
            );
            total_interest = total_interest
                .checked_add(interest)
                .ok_or(DefiError::ArithmeticOverflow)?;
        }

        self.total_debt = self
            .total_debt
            .checked_add(total_interest)
            .ok_or(DefiError::ArithmeticOverflow)?;
        self.last_accrual_ts = now_ts;
        Ok(total_interest)
    }

    /// 按仓位计算抵押率。
    fn calculate_collateral_ratio_bps(collateral_amount: u64, debt_amount: u64) -> u64 {
        if debt_amount == 0 {
            return u64::MAX;
        }
        ((collateral_amount as u128)
            .saturating_mul(BPS_DENOMINATOR as u128)
            .saturating_div(debt_amount as u128)) as u64
    }

    /// 乘除安全工具，避免中间过程溢出。
    fn mul_div_u64(a: u64, b: u64, denominator: u64) -> Result<u64, DefiError> {
        if denominator == 0 {
            return Err(DefiError::ArithmeticOverflow);
        }
        let value = (a as u128)
            .checked_mul(b as u128)
            .ok_or(DefiError::ArithmeticOverflow)?
            .checked_div(denominator as u128)
            .ok_or(DefiError::ArithmeticOverflow)?;
        u64::try_from(value).map_err(|_| DefiError::ArithmeticOverflow)
    }
}

#[cfg(test)]
mod tests {
    use super::{DefiError, InterestRateModel, LendingConfig, LendingPool, SECONDS_PER_YEAR};

    /// 验证抵押后可在健康阈值内借款。
    #[test]
    fn deposit_and_borrow_should_work() {
        let mut pool = LendingPool::new(LendingConfig::default(), 0);
        pool.deposit_collateral("alice", 200).expect("抵押应成功");

        let position = pool.borrow("alice", 100, 10).expect("借款应成功");
        assert_eq!(position.debt_amount, 100);
        assert_eq!(position.collateral_ratio_bps, 20_000);
    }

    /// 验证超额借款会被拒绝。
    #[test]
    fn over_borrow_should_fail() {
        let mut pool = LendingPool::new(LendingConfig::default(), 0);
        pool.deposit_collateral("alice", 100).expect("抵押应成功");

        let result = pool.borrow("alice", 100, 5);
        assert_eq!(
            result,
            Err(DefiError::CollateralRatioTooLow {
                actual: 10_000,
                required: 15_000,
            })
        );
    }

    /// 验证计息会增加总债务。
    #[test]
    fn interest_accrual_should_increase_debt() {
        let config = LendingConfig {
            interest_rate_model: InterestRateModel {
                base_rate_bps: 10_000,
                utilization_slope_bps: 0,
            },
            ..LendingConfig::default()
        };
        let mut pool = LendingPool::new(config, 0);
        pool.deposit_collateral("alice", 300).expect("抵押应成功");
        pool.borrow("alice", 100, 0).expect("借款应成功");

        let minted = pool
            .accrue_interest(SECONDS_PER_YEAR as i64)
            .expect("计息应成功");
        assert_eq!(minted, 100);
        assert_eq!(pool.total_debt, 200);
    }

    /// 验证还款会减少债务。
    #[test]
    fn repay_should_reduce_debt() {
        let mut pool = LendingPool::new(LendingConfig::default(), 0);
        pool.deposit_collateral("alice", 400).expect("抵押应成功");
        pool.borrow("alice", 120, 0).expect("借款应成功");

        let repaid = pool.repay("alice", 50, 100).expect("还款应成功");
        let position = pool.positions.get("alice").expect("仓位应存在");
        assert_eq!(repaid, 50);
        assert_eq!(position.debt_amount, 70);
    }

    /// 验证提取抵押会做健康度校验。
    #[test]
    fn unsafe_withdraw_should_fail() {
        let mut pool = LendingPool::new(LendingConfig::default(), 0);
        pool.deposit_collateral("alice", 150).expect("抵押应成功");
        pool.borrow("alice", 100, 0).expect("借款应成功");

        let result = pool.withdraw_collateral("alice", 10, 1);
        assert_eq!(
            result,
            Err(DefiError::CollateralRatioTooLow {
                actual: 14_000,
                required: 15_000,
            })
        );
    }

    /// 验证仓位恶化后可以被清算。
    #[test]
    fn liquidation_should_work_when_ratio_below_threshold() {
        let config = LendingConfig {
            interest_rate_model: InterestRateModel {
                base_rate_bps: 50_000,
                utilization_slope_bps: 0,
            },
            ..LendingConfig::default()
        };
        let mut pool = LendingPool::new(config, 0);
        pool.deposit_collateral("alice", 150).expect("抵押应成功");
        pool.borrow("alice", 100, 0).expect("借款应成功");
        pool.accrue_interest(SECONDS_PER_YEAR as i64)
            .expect("计息应成功");

        let outcome = pool.liquidate("alice", 80, SECONDS_PER_YEAR as i64 + 1);
        assert!(outcome.is_ok());
        let final_position = pool.positions.get("alice").expect("仓位应存在");
        assert!(final_position.debt_amount < 600);
    }
}
