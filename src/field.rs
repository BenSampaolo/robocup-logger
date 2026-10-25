use std::f64::consts::FRAC_PI_2;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Block, Paragraph,
        canvas::{Canvas, Circle, Context, Line as Segment, Points, Rectangle},
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    presentation::{ACCENT, MUTED, clipped_line, solid_panel},
    telemetry::Telemetry,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FieldDimensions {
    pub line_width: f64,
    pub length: f64,
    pub width: f64,
    pub goal_depth: f64,
    pub goal_inner_width: f64,
    pub goal_post_diameter: f64,
    pub goal_box_area_length: f64,
    pub goal_box_area_width: f64,
    pub penalty_area_length: f64,
    pub penalty_area_width: f64,
    pub penalty_marker_distance: f64,
    pub center_circle_diameter: f64,
    pub border_strip_width: f64,
    pub corner_arc_radius: f64,
    pub penalty_marker_size: f64,
}

impl Default for FieldDimensions {
    fn default() -> Self {
        Self {
            line_width: 0.09,
            length: 21.91,
            width: 13.93,
            goal_depth: 1.0,
            goal_inner_width: 3.0,
            goal_post_diameter: 0.04,
            goal_box_area_length: 1.92,
            goal_box_area_width: 4.9,
            penalty_area_length: 4.91,
            penalty_area_width: 7.9,
            penalty_marker_distance: 3.45,
            center_circle_diameter: 3.9,
            border_strip_width: 1.05,
            corner_arc_radius: 0.0,
            penalty_marker_size: 0.3,
        }
    }
}

impl FieldDimensions {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value, zero_allowed) in [
            ("line_width", self.line_width, false),
            ("length", self.length, false),
            ("width", self.width, false),
            ("goal_depth", self.goal_depth, false),
            ("goal_inner_width", self.goal_inner_width, false),
            ("goal_post_diameter", self.goal_post_diameter, false),
            ("goal_box_area_length", self.goal_box_area_length, false),
            ("goal_box_area_width", self.goal_box_area_width, false),
            ("penalty_area_length", self.penalty_area_length, false),
            ("penalty_area_width", self.penalty_area_width, false),
            (
                "penalty_marker_distance",
                self.penalty_marker_distance,
                false,
            ),
            ("center_circle_diameter", self.center_circle_diameter, false),
            ("border_strip_width", self.border_strip_width, true),
            ("corner_arc_radius", self.corner_arc_radius, true),
            ("penalty_marker_size", self.penalty_marker_size, false),
        ] {
            if !(zero_allowed && value == 0.0) && !(0.000_001..=1_000.0).contains(&value) {
                return Err(format!(
                    "{name} must be between 0.000001 and 1000 metres{}",
                    if zero_allowed { " (or zero)" } else { "" }
                ));
            }
        }
        if self.goal_box_area_length >= self.penalty_area_length
            || self.penalty_area_length >= self.length / 2.0
            || self.goal_box_area_width >= self.penalty_area_width
            || self.penalty_area_width >= self.width
        {
            return Err(
                "Goal boxes must fit inside penalty areas, which must fit within each field half"
                    .into(),
            );
        }
        if self.goal_inner_width + 2.0 * self.goal_post_diameter >= self.goal_box_area_width
            || self.goal_post_diameter >= self.goal_depth
            || self.goal_post_diameter >= self.goal_inner_width
            || self.goal_depth >= self.length / 2.0
        {
            return Err("Goal opening/posts must fit inside the goal box and goal depth".into());
        }
        let marker_radius = self.penalty_marker_size / 2.0;
        if self.penalty_marker_distance - marker_radius <= self.goal_box_area_length
            || self.penalty_marker_distance + marker_radius >= self.penalty_area_length
            || self.penalty_marker_size >= self.penalty_area_width
        {
            return Err(
                "Penalty markers must fit between the goal box and penalty-area edge".into(),
            );
        }
        if self.center_circle_diameter >= self.width
            || self.center_circle_diameter / 2.0 >= self.length / 2.0 - self.penalty_area_length
            || self.penalty_marker_size >= self.center_circle_diameter
            || self.corner_arc_radius >= self.width.min(self.length) / 2.0
        {
            return Err(
                "Center circle, center marker, or corner arcs do not fit inside the field".into(),
            );
        }
        if [
            self.goal_box_area_length,
            self.goal_box_area_width,
            self.goal_depth,
            self.goal_inner_width,
            self.center_circle_diameter,
            self.penalty_marker_size,
        ]
        .into_iter()
        .any(|size| self.line_width >= size)
        {
            return Err("line_width must be smaller than the field markings".into());
        }
        let (width, height) = self.view_size();
        if !width.is_finite() || !height.is_finite() || !(width / height).is_normal() {
            return Err("Field and border-strip dimensions exceed the drawable range".into());
        }
        Ok(())
    }

    fn view_size(&self) -> (f64, f64) {
        (
            self.width + 2.0 * self.border_strip_width + self.line_width,
            self.length
                + 2.0
                    * self
                        .border_strip_width
                        .max(self.goal_depth + self.goal_post_diameter / 2.0)
                + self.line_width,
        )
    }
}

const ROBOT_COLORS: [Color; 5] = [
    Color::LightCyan,
    Color::LightMagenta,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightRed,
];
const FIELD_COLOR: Color = Color::Rgb(146, 176, 153);

pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    dimensions: &FieldDimensions,
    telemetry: &Telemetry,
) {
    let inner = solid_panel(
        frame,
        area,
        Block::default().title(" FIELD · attack ↑ "),
        ACCENT,
    );
    let status = telemetry.status();
    if inner.width == 0 || inner.height == 0 {
        frame.render_widget(Paragraph::new(status), area.intersection(frame.area()));
        return;
    }
    let plot_height = inner.height - 1;
    let (minimum_width, minimum_height) = dimensions.view_size();
    let dots_x = f64::from(inner.width) * 2.0 - 1.0;
    let dots_y = f64::from(plot_height.max(1)) * 4.0 - 1.0;
    let pixel = (minimum_width / dots_x).max(minimum_height / dots_y);
    let (world_width, world_height) = (pixel * dots_x, pixel * dots_y);
    let mut spans: Vec<_> = telemetry
        .robots
        .iter()
        .enumerate()
        .map(|(index, robot)| {
            let fresh = robot.as_ref().is_some_and(|robot| robot.fresh());
            let mut label = (index + 1).to_string();
            if !fresh {
                label.push('?');
            } else if let Some(robot) = robot {
                if robot.y.abs() > world_width / 2.0 || robot.x.abs() > world_height / 2.0 {
                    label.push('↗');
                } else if robot.fallen {
                    label.push('↓');
                }
            }
            label.push(' ');
            Span::styled(
                label,
                Style::default().fg(if fresh { ROBOT_COLORS[index] } else { MUTED }),
            )
        })
        .collect();
    spans.push(Span::styled(
        status.clone(),
        Style::default().fg(Color::Gray),
    ));
    let legend = Line::from(spans);
    let legend = if legend.width() <= usize::from(inner.width) {
        legend
    } else {
        Line::from(status)
    };
    frame.render_widget(
        Paragraph::new(legend),
        Rect::new(inner.x, inner.y + plot_height, inner.width, 1),
    );
    if plot_height == 0 {
        return;
    }
    let plot = Rect::new(inner.x, inner.y, inner.width, plot_height);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .background_color(Color::Rgb(13, 29, 25))
        .x_bounds([-world_width / 2.0, world_width / 2.0])
        .y_bounds([-world_height / 2.0, world_height / 2.0])
        .paint(|ctx| {
            let strokes = (dimensions.line_width / pixel).ceil().max(1.0) as usize;
            for stroke in 0..strokes {
                let offset = if strokes == 1 {
                    0.0
                } else {
                    dimensions.line_width * (stroke as f64 / (strokes - 1) as f64 - 0.5)
                };
                draw_markings(ctx, dimensions, offset);
            }
            draw_goals(ctx, dimensions);
            ctx.layer();
            for (index, robot) in telemetry.robots.iter().enumerate() {
                let Some(robot) = robot.as_ref().filter(|robot| robot.fresh()) else {
                    continue;
                };
                let color = ROBOT_COLORS[index];
                let (sin, cos) = (robot.heading + FRAC_PI_2).sin_cos();
                let size = 0.45_f64.max(pixel * 2.0);
                let vertices =
                    [(1.0, 0.0), (-0.65, 0.65), (-0.65, -0.65)].map(|(forward, left)| {
                        (
                            -robot.y + size * (forward * cos - left * sin),
                            robot.x + size * (forward * sin + left * cos),
                        )
                    });
                for edge in 0..3 {
                    let (x1, y1) = vertices[edge];
                    let (x2, y2) = vertices[(edge + 1) % 3];
                    clipped_line(
                        ctx,
                        (x1, y1),
                        (x2, y2),
                        (world_width / 2.0, world_height / 2.0),
                        color,
                    );
                }
                if let Some((x, y)) = robot
                    .ball
                    .filter(|_| robot.ball_age + robot.received.elapsed().as_secs_f64() <= 5.0)
                {
                    ctx.draw(&Circle {
                        x: -y,
                        y: x,
                        radius: 0.15_f64.max(pixel),
                        color,
                    });
                }
                ctx.layer();
            }
        });
    frame.render_widget(canvas, plot);
    crate::presentation::grass_stripes(
        frame,
        plot,
        [dimensions.width, dimensions.length],
        [world_width, world_height],
        ratatui::layout::Direction::Horizontal,
    );
    if plot.height < 3 || plot.width < 3 {
        frame.render_widget(
            Paragraph::new("Enlarge map").style(Style::default().fg(MUTED)),
            plot,
        );
    }
}

fn draw_markings(ctx: &mut Context<'_>, dimensions: &FieldDimensions, offset: f64) {
    let (half_width, half_length) = (dimensions.width / 2.0, dimensions.length / 2.0);
    ctx.draw(&Rectangle {
        x: -half_width - offset,
        y: -half_length - offset,
        width: dimensions.width + 2.0 * offset,
        height: dimensions.length + 2.0 * offset,
        color: FIELD_COLOR,
    });
    ctx.draw(&Segment::new(
        -half_width,
        offset,
        half_width,
        offset,
        FIELD_COLOR,
    ));
    ctx.draw(&Circle {
        x: 0.0,
        y: 0.0,
        radius: dimensions.center_circle_diameter / 2.0 + offset,
        color: FIELD_COLOR,
    });
    for side in [-1.0, 1.0] {
        for (width, depth) in [
            (
                dimensions.goal_box_area_width,
                dimensions.goal_box_area_length,
            ),
            (
                dimensions.penalty_area_width,
                dimensions.penalty_area_length,
            ),
        ] {
            let y = if side > 0.0 {
                half_length - depth
            } else {
                -half_length
            };
            ctx.draw(&Rectangle {
                x: -width / 2.0 - offset,
                y: y - offset,
                width: width + 2.0 * offset,
                height: depth + 2.0 * offset,
                color: FIELD_COLOR,
            });
        }
    }
    for y in [
        0.0,
        half_length - dimensions.penalty_marker_distance,
        -half_length + dimensions.penalty_marker_distance,
    ] {
        let radius = dimensions.penalty_marker_size / 2.0;
        ctx.draw(&Segment::new(
            -radius,
            y + offset,
            radius,
            y + offset,
            FIELD_COLOR,
        ));
        ctx.draw(&Segment::new(
            offset,
            y - radius,
            offset,
            y + radius,
            FIELD_COLOR,
        ));
    }
    if dimensions.corner_arc_radius > 0.0 {
        let radius = (dimensions.corner_arc_radius + offset).max(0.0);
        for x_side in [-1.0, 1.0] {
            for y_side in [-1.0, 1.0] {
                let points: Vec<_> = (0..=90)
                    .map(|angle| {
                        let (sin, cos) = f64::from(angle).to_radians().sin_cos();
                        (
                            x_side * (half_width - radius * cos),
                            y_side * (half_length - radius * sin),
                        )
                    })
                    .collect();
                ctx.draw(&Points {
                    coords: &points,
                    color: FIELD_COLOR,
                });
            }
        }
    }
}

fn draw_goals(ctx: &mut Context<'_>, dimensions: &FieldDimensions) {
    let post_x = (dimensions.goal_inner_width + dimensions.goal_post_diameter) / 2.0;
    for side in [-1.0, 1.0] {
        let goal_line = side * dimensions.length / 2.0;
        ctx.draw(&Rectangle {
            x: -post_x,
            y: if side > 0.0 {
                goal_line
            } else {
                goal_line - dimensions.goal_depth
            },
            width: post_x * 2.0,
            height: dimensions.goal_depth,
            color: FIELD_COLOR,
        });
        for x in [-post_x, post_x] {
            ctx.draw(&Circle {
                x,
                y: goal_line,
                radius: dimensions.goal_post_diameter / 2.0,
                color: Color::White,
            });
        }
    }
}
