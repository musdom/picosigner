use picosigner_board::TouchPoint;

#[derive(Clone, Copy)]
pub enum HoldAction {
    Backup,
    Regenerate,
    Sign,
    Scan,
    Descriptor,
    Brightness,
    Back,
}

pub const HOME_SCAN_BUTTON: (i32, i32, u32, u32) = (16, 112, 208, 40);
pub const HOME_DESCRIPTOR_BUTTON: (i32, i32, u32, u32) = (16, 164, 208, 40);
pub const HOME_NEW_WALLET_BUTTON: (i32, i32, u32, u32) = (16, 216, 208, 40);
pub const HOME_NEW_WALLET_BAR_Y: i32 = 270;
pub const DESCRIPTOR_BACK_BUTTON: (i32, i32, u32, u32) = (0, 296, 240, 24);
pub const REVIEW_BACK_BUTTON: (i32, i32, u32, u32) = (180, 0, 60, 32);
pub const REVIEW_SIGN_BUTTON: (i32, i32, u32, u32) = (16, 288, 208, 32);
pub const REVIEW_SIGN_BAR_Y: i32 = 304;
pub const SCAN_CANCEL_BUTTON: (u16, u16, u16, u16) = (174, 0, 66, 24);
pub const SCAN_ZOOM_BUTTON: (u16, u16, u16, u16) = (70, 280, 100, 40);
pub const SCAN_PROGRESS_BAR: (i32, i32, u32, u32) = (12, 263, 216, 11);

pub fn portrait_touch(point: TouchPoint) -> (u16, u16) {
    // CST816D coordinates already use the panel's native 240x320 portrait
    // axes, matching every application view.
    (point.x, point.y)
}

pub fn home_action(point: TouchPoint) -> Option<HoldAction> {
    let (x, y) = portrait_touch(point);
    if in_button(x, y, HOME_SCAN_BUTTON) {
        Some(HoldAction::Scan)
    } else if in_button(x, y, HOME_DESCRIPTOR_BUTTON) {
        Some(HoldAction::Descriptor)
    } else if in_button(x, y, HOME_NEW_WALLET_BUTTON) {
        Some(HoldAction::Regenerate)
    } else {
        None
    }
}

pub fn descriptor_action(point: TouchPoint) -> Option<HoldAction> {
    let (x, y) = portrait_touch(point);
    if in_button(x, y, DESCRIPTOR_BACK_BUTTON) {
        Some(HoldAction::Back)
    } else if y < DESCRIPTOR_BACK_BUTTON.1 as u16 {
        Some(HoldAction::Brightness)
    } else {
        None
    }
}

pub fn review_action(point: TouchPoint) -> Option<HoldAction> {
    let (x, y) = portrait_touch(point);
    if in_button(x, y, REVIEW_BACK_BUTTON) {
        Some(HoldAction::Back)
    } else if in_button(x, y, REVIEW_SIGN_BUTTON) {
        Some(HoldAction::Sign)
    } else {
        None
    }
}

pub fn bottom_back_action(point: TouchPoint) -> Option<HoldAction> {
    let (_, y) = portrait_touch(point);
    (y >= DESCRIPTOR_BACK_BUTTON.1 as u16).then_some(HoldAction::Back)
}

pub fn in_button(x: u16, y: u16, button: (i32, i32, u32, u32)) -> bool {
    let (left, top, width, height) = button;
    let right = left + width as i32;
    let bottom = top + height as i32;
    (x as i32) >= left && (x as i32) < right && (y as i32) >= top && (y as i32) < bottom
}

pub fn point_in_u16_button(point: TouchPoint, button: (u16, u16, u16, u16)) -> bool {
    let (x, y, width, height) = button;
    point.x >= x
        && point.x < x.saturating_add(width)
        && point.y >= y
        && point.y < y.saturating_add(height)
}

pub fn hold_bar_y(action: HoldAction) -> Option<i32> {
    match action {
        HoldAction::Backup => Some(258),
        HoldAction::Regenerate => Some(HOME_NEW_WALLET_BAR_Y),
        HoldAction::Sign => Some(REVIEW_SIGN_BAR_Y),
        HoldAction::Scan | HoldAction::Descriptor | HoldAction::Brightness | HoldAction::Back => {
            None
        }
    }
}
