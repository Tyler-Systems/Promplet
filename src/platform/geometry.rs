//! Pure placement math shared by the platform backends.

use crate::model::Orientation;

/// Edge coordinates of a rectangle in screen space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    #[cfg_attr(
        windows,
        allow(
            dead_code,
            reason = "only the macOS backend builds Bounds from FLTK sizes"
        )
    )]
    pub fn from_position_and_size(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        }
    }
}

pub fn editor_position(
    work_area: &Bounds,
    anchor: &Bounds,
    width: i32,
    height: i32,
    gap: i32,
    orientation: Orientation,
) -> (i32, i32) {
    let max_x = (work_area.right - width).max(work_area.left);
    let max_y = (work_area.bottom - height).max(work_area.top);
    let gap = gap.max(0);

    match orientation {
        Orientation::Horizontal => {
            let x = (anchor.right - width).clamp(work_area.left, max_x);

            let above = anchor.top - gap - height;
            let below = anchor.bottom + gap;
            let preferred_y = if above >= work_area.top {
                above
            } else if below + height <= work_area.bottom {
                below
            } else {
                above
            };

            (x, preferred_y.clamp(work_area.top, max_y))
        }
        Orientation::Vertical => {
            let left = anchor.left - gap - width;
            let right = anchor.right + gap;
            let preferred_x = if left >= work_area.left {
                left
            } else if right + width <= work_area.right {
                right
            } else {
                left
            };
            let y = (anchor.bottom - height).clamp(work_area.top, max_y);

            (preferred_x.clamp(work_area.left, max_x), y)
        }
    }
}

pub fn visible_position(work_area: &Bounds, window: &Bounds, margin: i32) -> (i32, i32) {
    let width = window.right - window.left;
    let height = window.bottom - window.top;
    let (min_x, max_x) = axis_bounds(work_area.left, work_area.right, width, margin);
    let (min_y, max_y) = axis_bounds(work_area.top, work_area.bottom, height, margin);

    (
        window.left.clamp(min_x, max_x),
        window.top.clamp(min_y, max_y),
    )
}

fn axis_bounds(start: i32, end: i32, size: i32, margin: i32) -> (i32, i32) {
    let margin = margin.max(0);
    let inset_start = start.saturating_add(margin);
    let inset_end = end.saturating_sub(margin);

    if inset_end.saturating_sub(inset_start) >= size {
        (inset_start, inset_end - size)
    } else {
        (start, (end - size).max(start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_is_right_aligned_above_anchor() {
        let work_area = Bounds {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1392,
        };
        let anchor = Bounds {
            left: 2270,
            top: 1349,
            right: 2548,
            bottom: 1380,
        };

        assert_eq!(
            editor_position(&work_area, &anchor, 520, 390, 8, Orientation::Horizontal),
            (2028, 951)
        );
    }

    #[test]
    fn editor_stays_inside_monitor_work_area() {
        let work_area = Bounds {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        let anchor = Bounds {
            left: -1915,
            top: 2,
            right: -1700,
            bottom: 33,
        };

        assert_eq!(
            editor_position(&work_area, &anchor, 520, 390, 8, Orientation::Horizontal),
            (-1920, 41)
        );
    }

    #[test]
    fn vertical_editor_is_left_and_bottom_aligned() {
        let work_area = Bounds {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1392,
        };
        let anchor = Bounds {
            left: 2517,
            top: 800,
            right: 2548,
            bottom: 1380,
        };

        assert_eq!(
            editor_position(&work_area, &anchor, 520, 390, 8, Orientation::Vertical),
            (1989, 990)
        );
    }

    #[test]
    fn hidden_strip_is_recovered_into_the_work_area() {
        let work_area = Bounds {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1392,
        };
        let hidden_strip = Bounds {
            left: 2077,
            top: 1403,
            right: 2355,
            bottom: 1434,
        };

        assert_eq!(
            visible_position(&work_area, &hidden_strip, 12),
            (2077, 1349)
        );
    }
}
