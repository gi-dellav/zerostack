use crate::ui::slash::{SlashCtx, write_result};

pub fn write_help(ctx: &mut SlashCtx<'_>) {
    write_result(ctx.renderer, "  /advisor               show advisor status");
}
