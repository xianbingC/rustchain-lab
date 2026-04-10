use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// NFT 资产信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NftToken {
    /// 资产唯一标识。
    pub token_id: String,
    /// 当前持有人地址。
    pub owner: String,
    /// 资产名称。
    pub name: String,
    /// 资产描述。
    pub description: String,
    /// 图片链接。
    pub image_url: String,
}

/// 市场挂单状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListingStatus {
    /// 挂单可被购买。
    Active,
    /// 挂单已成交。
    Sold,
    /// 挂单被取消。
    Cancelled,
}

/// NFT 挂单信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NftListing {
    /// 挂单 ID。
    pub listing_id: String,
    /// NFT 资产 ID。
    pub token_id: String,
    /// 卖家地址。
    pub seller: String,
    /// 标价（最小单位）。
    pub price: u64,
    /// 挂单状态。
    pub status: ListingStatus,
}

/// NFT 市场错误。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum NftError {
    /// 必填参数为空。
    #[error("参数不能为空: {0}")]
    MissingField(&'static str),
    /// 金额必须大于 0。
    #[error("价格必须大于 0")]
    InvalidPrice,
    /// 资产不存在。
    #[error("NFT 不存在: token_id={token_id}")]
    TokenNotFound { token_id: String },
    /// 当前用户不是资产持有人。
    #[error("当前用户不是 NFT 持有人")]
    NotTokenOwner,
    /// 资产已在有效挂单中。
    #[error("NFT 已经在售")]
    TokenAlreadyListed,
    /// 挂单不存在。
    #[error("挂单不存在: listing_id={listing_id}")]
    ListingNotFound { listing_id: String },
    /// 挂单已结束，不可再操作。
    #[error("挂单状态不可操作: status={status:?}")]
    ListingClosed { status: ListingStatus },
    /// 买家不能是卖家本人。
    #[error("不能购买自己的挂单")]
    SelfPurchaseNotAllowed,
}

/// 购买结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PurchaseOutcome {
    /// 成交挂单 ID。
    pub listing_id: String,
    /// 购买人地址。
    pub buyer: String,
    /// 成交资产信息。
    pub token: NftToken,
    /// 成交价格。
    pub paid_price: u64,
}

/// NFT 市场核心状态。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct NftMarketplace {
    /// 全量 NFT 资产索引。
    pub tokens: HashMap<String, NftToken>,
    /// 全量挂单索引。
    pub listings: HashMap<String, NftListing>,
    /// 令牌 ID 自增序号（原型阶段使用）。
    next_token_seq: u64,
    /// 挂单 ID 自增序号（原型阶段使用）。
    next_listing_seq: u64,
}

impl NftMarketplace {
    /// 创建空市场实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 铸造 NFT 到指定持有人。
    pub fn mint(
        &mut self,
        owner: &str,
        name: &str,
        description: &str,
        image_url: &str,
    ) -> Result<NftToken, NftError> {
        if owner.trim().is_empty() {
            return Err(NftError::MissingField("owner"));
        }
        if name.trim().is_empty() {
            return Err(NftError::MissingField("name"));
        }
        if image_url.trim().is_empty() {
            return Err(NftError::MissingField("image_url"));
        }

        self.next_token_seq = self.next_token_seq.saturating_add(1);
        let token_id = format!("nft-{}", self.next_token_seq);
        let token = NftToken {
            token_id: token_id.clone(),
            owner: owner.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            image_url: image_url.to_string(),
        };
        self.tokens.insert(token_id, token.clone());
        Ok(token)
    }

    /// 将 NFT 上架到市场。
    pub fn list(
        &mut self,
        seller: &str,
        token_id: &str,
        price: u64,
    ) -> Result<NftListing, NftError> {
        if seller.trim().is_empty() {
            return Err(NftError::MissingField("seller"));
        }
        if token_id.trim().is_empty() {
            return Err(NftError::MissingField("token_id"));
        }
        if price == 0 {
            return Err(NftError::InvalidPrice);
        }

        let token = self
            .tokens
            .get(token_id)
            .ok_or_else(|| NftError::TokenNotFound {
                token_id: token_id.to_string(),
            })?;
        if token.owner != seller {
            return Err(NftError::NotTokenOwner);
        }

        if self
            .listings
            .values()
            .any(|listing| listing.token_id == token_id && listing.status == ListingStatus::Active)
        {
            return Err(NftError::TokenAlreadyListed);
        }

        self.next_listing_seq = self.next_listing_seq.saturating_add(1);
        let listing = NftListing {
            listing_id: format!("listing-{}", self.next_listing_seq),
            token_id: token_id.to_string(),
            seller: seller.to_string(),
            price,
            status: ListingStatus::Active,
        };
        self.listings
            .insert(listing.listing_id.clone(), listing.clone());
        Ok(listing)
    }

    /// 取消挂单。
    pub fn cancel_listing(
        &mut self,
        seller: &str,
        listing_id: &str,
    ) -> Result<NftListing, NftError> {
        if seller.trim().is_empty() {
            return Err(NftError::MissingField("seller"));
        }
        if listing_id.trim().is_empty() {
            return Err(NftError::MissingField("listing_id"));
        }

        let listing =
            self.listings
                .get_mut(listing_id)
                .ok_or_else(|| NftError::ListingNotFound {
                    listing_id: listing_id.to_string(),
                })?;

        if listing.seller != seller {
            return Err(NftError::NotTokenOwner);
        }
        if listing.status != ListingStatus::Active {
            return Err(NftError::ListingClosed {
                status: listing.status,
            });
        }

        listing.status = ListingStatus::Cancelled;
        Ok(listing.clone())
    }

    /// 购买挂单资产，并完成 NFT 所有权转移。
    pub fn buy(&mut self, buyer: &str, listing_id: &str) -> Result<PurchaseOutcome, NftError> {
        if buyer.trim().is_empty() {
            return Err(NftError::MissingField("buyer"));
        }
        if listing_id.trim().is_empty() {
            return Err(NftError::MissingField("listing_id"));
        }

        let listing =
            self.listings
                .get_mut(listing_id)
                .ok_or_else(|| NftError::ListingNotFound {
                    listing_id: listing_id.to_string(),
                })?;
        if listing.status != ListingStatus::Active {
            return Err(NftError::ListingClosed {
                status: listing.status,
            });
        }
        if listing.seller == buyer {
            return Err(NftError::SelfPurchaseNotAllowed);
        }

        let token =
            self.tokens
                .get_mut(&listing.token_id)
                .ok_or_else(|| NftError::TokenNotFound {
                    token_id: listing.token_id.clone(),
                })?;
        token.owner = buyer.to_string();
        listing.status = ListingStatus::Sold;

        Ok(PurchaseOutcome {
            listing_id: listing.listing_id.clone(),
            buyer: buyer.to_string(),
            token: token.clone(),
            paid_price: listing.price,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ListingStatus, NftError, NftMarketplace};

    /// 验证铸造会生成唯一 NFT ID。
    #[test]
    fn mint_should_generate_unique_token_id() {
        let mut market = NftMarketplace::new();
        let t1 = market
            .mint("alice", "Sunset", "digital art", "https://img/1.png")
            .expect("铸造应成功");
        let t2 = market
            .mint("alice", "River", "digital art", "https://img/2.png")
            .expect("铸造应成功");

        assert_ne!(t1.token_id, t2.token_id);
        assert_eq!(market.tokens.len(), 2);
    }

    /// 验证非持有人不能挂单。
    #[test]
    fn non_owner_should_not_list_token() {
        let mut market = NftMarketplace::new();
        let token = market
            .mint("alice", "Sunset", "digital art", "https://img/1.png")
            .expect("铸造应成功");

        let result = market.list("bob", &token.token_id, 100);
        assert_eq!(result, Err(NftError::NotTokenOwner));
    }

    /// 验证挂单后可被购买并转移所有权。
    #[test]
    fn buy_should_transfer_ownership() {
        let mut market = NftMarketplace::new();
        let token = market
            .mint("alice", "Sunset", "digital art", "https://img/1.png")
            .expect("铸造应成功");
        let listing = market
            .list("alice", &token.token_id, 188)
            .expect("挂单应成功");

        let outcome = market.buy("bob", &listing.listing_id).expect("购买应成功");
        assert_eq!(outcome.paid_price, 188);
        assert_eq!(outcome.token.owner, "bob");
        assert_eq!(
            market
                .listings
                .get(&listing.listing_id)
                .expect("挂单应存在")
                .status,
            ListingStatus::Sold
        );
    }

    /// 验证卖家可取消活跃挂单。
    #[test]
    fn seller_should_cancel_active_listing() {
        let mut market = NftMarketplace::new();
        let token = market
            .mint("alice", "Sunset", "digital art", "https://img/1.png")
            .expect("铸造应成功");
        let listing = market
            .list("alice", &token.token_id, 200)
            .expect("挂单应成功");

        let cancelled = market
            .cancel_listing("alice", &listing.listing_id)
            .expect("取消应成功");
        assert_eq!(cancelled.status, ListingStatus::Cancelled);
    }

    /// 验证已关闭挂单不可再次购买。
    #[test]
    fn closed_listing_should_not_be_buyable() {
        let mut market = NftMarketplace::new();
        let token = market
            .mint("alice", "Sunset", "digital art", "https://img/1.png")
            .expect("铸造应成功");
        let listing = market
            .list("alice", &token.token_id, 200)
            .expect("挂单应成功");
        market
            .cancel_listing("alice", &listing.listing_id)
            .expect("取消应成功");

        let result = market.buy("bob", &listing.listing_id);
        assert_eq!(
            result,
            Err(NftError::ListingClosed {
                status: ListingStatus::Cancelled
            })
        );
    }

    /// 验证卖家不能购买自己的挂单。
    #[test]
    fn seller_should_not_buy_own_listing() {
        let mut market = NftMarketplace::new();
        let token = market
            .mint("alice", "Sunset", "digital art", "https://img/1.png")
            .expect("铸造应成功");
        let listing = market
            .list("alice", &token.token_id, 200)
            .expect("挂单应成功");

        let result = market.buy("alice", &listing.listing_id);
        assert_eq!(result, Err(NftError::SelfPurchaseNotAllowed));
    }
}
