//! 排版档位：整个 UI 的字号/字重唯一事实源。
//! 层级三轴：标题档（大、semibold）> 正文档（16px、400）> 辅助档（小、muted 色）。
//! 相邻档位至少两轴不同，保证一眼可辨。

use gpui_kit::gpui::{FontWeight, Pixels, px};

/// 界面上的文本角色。角色名描述用途，不描述数值。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeRole {
    /// 应用标题（“tailash”）。
    Title,
    /// 区块/组标题（组名、区块名）。
    Section,
    /// 正文与控件标签（开关标签、模式行、区块说明行）。
    Body,
    /// 辅助信息（延迟、类型、UDP、计数、状态消息）。
    Meta,
}

impl TypeRole {
    /// 字号。辅助 < 正文 < 区块 < 标题。
    pub fn size(self) -> Pixels {
        match self {
            TypeRole::Title => px(22.),
            TypeRole::Section => px(15.),
            TypeRole::Body => px(16.),
            TypeRole::Meta => px(13.),
        }
    }

    /// 字重。标题/区块档 semibold，正文/辅助档 normal。
    pub fn weight(self) -> FontWeight {
        match self {
            TypeRole::Title | TypeRole::Section => FontWeight::SEMIBOLD,
            TypeRole::Body | TypeRole::Meta => FontWeight::NORMAL,
        }
    }

    /// 行高按字号反向缩放宽裕度：字号越大行高比例越紧。
    pub fn line_height(self) -> Pixels {
        match self {
            TypeRole::Title => px(28.),
            TypeRole::Section => px(22.),
            TypeRole::Body => px(24.),
            TypeRole::Meta => px(20.),
        }
    }

    /// 层级可辨性：与相邻档位在 (size, weight, 行高) 至少两轴不同。
    pub fn differs_from_adjacent(self) -> bool {
        let ranks = [Self::Title, Self::Section, Self::Body, Self::Meta];
        let Some(self_rank) = ranks.iter().position(|r| r == &self) else {
            return false;
        };
        let axes = |a: Self, b: Self| {
            (a.size() != b.size()) as u8
                + (a.weight() != b.weight()) as u8
                + (a.line_height() != b.line_height()) as u8
        };
        let prev_ok = self_rank == 0 || axes(ranks[self_rank - 1], self) >= 2;
        let next_ok = self_rank + 1 == ranks.len() || axes(self, ranks[self_rank + 1]) >= 2;
        prev_ok && next_ok
    }
}

#[cfg(test)]
mod tests {
    use super::TypeRole;

    #[test]
    fn roles_form_a_discernible_scale() {
        for role in [
            TypeRole::Title,
            TypeRole::Section,
            TypeRole::Body,
            TypeRole::Meta,
        ] {
            assert!(
                role.differs_from_adjacent(),
                "{role:?} 与相邻档位在两轴内不可辨"
            );
        }
    }

    #[test]
    fn adjacent_roles_are_never_equal() {
        assert_ne!(TypeRole::Title.size(), TypeRole::Section.size());
        assert_ne!(TypeRole::Section.size(), TypeRole::Body.size());
        assert_ne!(TypeRole::Body.size(), TypeRole::Meta.size());
        assert_ne!(TypeRole::Section.weight(), TypeRole::Body.weight());
    }
}
