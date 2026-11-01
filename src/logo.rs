// SPDX-License-Identifier: GPL-3.0-or-later
//! The tileIDE logo (`design/logo.png`, traced with potrace) as one even-odd path, so
//! the rail draws it in any theme colour and the face stays a hole. The same path is
//! in `data/dev.tileide.TileIDE.svg`.

use gtk4::cairo::{Context, FillRule};

/// Absolute M/L/C/Z commands in a 1024×1024 box.
const PATH: &str = "M811.5 133.1C811.2 134.4 808.1 146.5 804.7 160L798.5 184.4L772.3 190.9C757.8 194.5 746 197.7 746 198C746 198.3 757.8 201.5 772.3 205.1L798.5 211.6L805.2 237.9C808.8 252.3 812 264.4 812.3 264.6C812.5 264.9 815.6 253.7 819 239.8C822.5 225.9 825.6 213.8 826 213C826.5 212 835.8 209.3 852.9 205C867.3 201.5 879 198.3 879 198C879 197.7 867.2 194.5 852.9 191C831.2 185.6 826.6 184.2 825.9 182.5C825.4 181.4 822.3 169.5 818.9 156C815.6 142.6 812.7 131.4 812.5 131.1C812.3 130.9 811.8 131.8 811.5 133.1ZM687 209C677.5 211.5 665.1 217.6 657.9 223.5C653.3 227.1 454.5 462.3 435.1 487C415.1 512.4 414.2 548.3 432.8 575.7C453.1 605.6 495.2 616.5 527.5 600.3C534.9 596.6 543.9 589.9 549 584.4C561.1 571.3 766 326.1 769.2 320.8C785.5 294.2 783.1 259.1 763.2 234.4C745.6 212.6 714 202.1 687 209ZM584.7 366.6C594.4 371.3 595.9 384.2 587.5 392C585.2 394.2 583.9 394.5 578 394.5C572.1 394.5 570.8 394.2 568.5 392C555.2 379.6 568.4 358.9 584.7 366.6ZM636.8 410.4C647.8 416.1 648.2 432.3 637.5 437.4C623.6 444.1 609.7 429.8 617.3 416.6C621.1 409.9 630.2 407 636.8 410.4ZM547.2 422C549.2 423.5 549.9 425.3 550.5 429.7C551.7 439.3 554.7 445.9 560.4 451.5C566.6 457.6 570.5 459 583.8 459.7C594.9 460.3 597 461.4 597 466.7C597 471.5 593.9 473 583.7 473C568.4 473 558.2 469.3 549.8 460.7C539.8 450.4 533.2 425.8 539.2 421.6C542.2 419.5 544.2 419.6 547.2 422ZM292.4 497.4C271 502.6 260.1 526.1 270 545.6L273.5 552.5L374.5 637.3C430.1 683.9 483.6 728.8 493.5 737.2C515 755.2 519.1 757.4 530.5 757.5C559.6 757.6 575.5 722.7 556.3 701.3C552.1 696.7 324.6 505.7 318.4 501.7C310.9 496.7 301.5 495.2 292.4 497.4ZM349.7 652C343.8 653.5 338.2 656.8 333.7 661.3C324.8 670.3 288.6 714.6 286.6 718.9C283.4 726 283.6 739.7 287.1 746.8L289.7 752L286.1 751.3C257.3 745.1 231 765.7 231 794.2C231 831.1 271.9 851.6 301.7 829.6C318.6 817.2 323.7 794.8 314.6 773.8C313.9 772.3 314.2 772.3 318.2 773.7C329.8 777.7 344.9 773.5 354 763.7C365.3 751.5 397 712.6 399.1 708.3C404 698.1 402.4 683.2 395.2 673.7C391.3 668.4 377.3 657.1 371.3 654.3C365.5 651.6 355.5 650.6 349.7 652ZM204.5 810.6C197.7 814.1 195 818.5 195 826C195 846 222.3 849.3 228.1 830C232 817.1 216.5 804.3 204.5 810.6Z";
/// The drawn shape's bounding box in `PATH` units: centre and largest side.
const CENTRE: (f64, f64) = (537.0, 491.0);
const EXTENT: f64 = 721.0;

/// Fills the logo, `size` px across, centred on (cx, cy), in the current source.
pub fn fill(cr: &Context, cx: f64, cy: f64, size: f64) -> Result<(), gtk4::cairo::Error> {
    let k = size / EXTENT;
    let at = |x: f64, y: f64| (cx + (x - CENTRE.0) * k, cy + (y - CENTRE.1) * k);
    cr.new_path();
    let mut numbers = Vec::with_capacity(6);
    let mut command = ' ';
    for token in tokens(PATH) {
        match token {
            Token::Command('Z') => cr.close_path(),
            Token::Command(c) => command = c,
            Token::Number(n) => {
                numbers.push(n);
                match (command, numbers.len()) {
                    ('M', 2) => {
                        let (x, y) = at(numbers[0], numbers[1]);
                        cr.move_to(x, y);
                    }
                    ('L', 2) => {
                        let (x, y) = at(numbers[0], numbers[1]);
                        cr.line_to(x, y);
                    }
                    ('C', 6) => {
                        let (a, b, c) = (at(numbers[0], numbers[1]), at(numbers[2], numbers[3]), at(numbers[4], numbers[5]));
                        cr.curve_to(a.0, a.1, b.0, b.1, c.0, c.1);
                    }
                    _ => continue,
                }
                numbers.clear();
            }
        }
    }
    cr.set_fill_rule(FillRule::EvenOdd);
    let result = cr.fill();
    cr.set_fill_rule(FillRule::Winding);
    result
}

enum Token {
    Command(char),
    Number(f64),
}

fn tokens(path: &str) -> impl Iterator<Item = Token> + '_ {
    let mut rest = path;
    std::iter::from_fn(move || {
        rest = rest.trim_start_matches([' ', ',']);
        let c = rest.chars().next()?;
        if c.is_ascii_alphabetic() {
            rest = &rest[1..];
            return Some(Token::Command(c));
        }
        let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
        let number = rest[..end].parse().ok()?;
        rest = &rest[end..];
        Some(Token::Number(number))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_parses_to_whole_commands() {
        let (mut commands, mut numbers) = (0, 0);
        for t in tokens(PATH) {
            match t {
                Token::Command(_) => commands += 1,
                Token::Number(_) => numbers += 1,
            }
        }
        assert!(commands > 20 && numbers % 2 == 0, "{commands} commands, {numbers} numbers");
    }
}
