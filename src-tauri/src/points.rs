use crate::db::Database;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointsOverview {
    pub total_earned: i64,
    pub total_spent: i64,
    pub balance: i64,
    pub streak_multiplier: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShopItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub cost: i64,
    pub unlocked: bool,
    pub equipped: bool,
    pub sprite_key: String,
    pub affordable: bool,
}

pub fn get_points_overview(db: &Database) -> PointsOverview {
    let total_earned = db.get_total_points().unwrap_or(0);
    let avatars = db.get_avatars().unwrap_or_default();
    let total_spent: i64 = avatars.iter()
        .filter(|a| a.unlocked && a.id != "default")
        .map(|a| a.cost)
        .sum();
    let balance = total_earned - total_spent;

    let streak = db.get_streak_info().ok();
    let multiplier = match streak {
        Some(s) if s.current >= 7 => 2.0,
        Some(s) if s.current >= 3 => 1.5,
        _ => 1.0,
    };

    PointsOverview {
        total_earned,
        total_spent,
        balance,
        streak_multiplier: multiplier,
    }
}

pub fn get_shop_items(db: &Database) -> Vec<ShopItem> {
    let overview = get_points_overview(db);
    let avatars = db.get_avatars().unwrap_or_default();

    avatars.into_iter().map(|a| {
        ShopItem {
            affordable: a.unlocked || overview.balance >= a.cost,
            id: a.id,
            name: a.name,
            description: a.description,
            cost: a.cost,
            unlocked: a.unlocked,
            equipped: a.equipped,
            sprite_key: a.sprite_key,
        }
    }).collect()
}
