//! The Makefile build system settings region view (`settings.view`):
//! - A native toolchain selector row for `make` (linked to Toolchains > Build Tools)
//! - A responsive canvas diagram illustrating how Lithe resolves `make` and
//!   discovers Makefile targets, matching the design language of Lithe's
//!   official build system plugins.

use lithe_plugin_sdk::view::{ViewEvent, ViewEventKind, ViewResult, ViewSurface};
use lithe_plugin_sdk::{Backend, Context, FunctionError, PluginError, SdkError};
use serde_json::{json, Map, Value};

const VIEW: &str = "settings.view";
const MAKEFILE_REGION: &str = "makefile";
const MAKE_TOOL: &str = "make";
const CANVAS_ID: &str = "makefile-resolution";

const DEFAULT_WIDTH: f64 = 340.0;
const MIN_WIDTH: f64 = 330.0;
const HEADER: f64 = 28.0;
const RAIL: f64 = 11.0;
const DOT: f64 = 20.0;
const STEP_X: f64 = 30.0;
const STEP_H: f64 = 64.0;
const STEP_GAP: f64 = 10.0;
const TITLE_MIDDLE: f64 = 17.0;
const LINE_X: f64 = 12.0;
const DISCOVERY_Y: f64 = HEADER + 3.0 * (STEP_H + STEP_GAP);
const USES_Y: f64 = DISCOVERY_Y + STEP_H + 14.0;
const HEIGHT: f64 = USES_Y + 22.0;
const CAPTION: f64 = 11.0;

pub fn register(backend: &mut Backend) -> Result<(), SdkError> {
    backend.view(VIEW, view)?;
    backend.suppress_success_diagnostics(VIEW)?;
    Ok(())
}

fn view(event: ViewEvent, _context: &Context) -> Result<ViewResult, FunctionError> {
    if !redraws(&event.kind) {
        return Ok(ViewResult::Unchanged);
    }
    Ok(ViewResult::View(render(&event)?))
}

fn redraws(kind: &ViewEventKind) -> bool {
    matches!(
        kind,
        ViewEventKind::Open | ViewEventKind::Settings | ViewEventKind::Resize
    )
}

fn render(event: &ViewEvent) -> Result<Vec<Value>, PluginError> {
    let ViewSurface::SettingRegion(region) = &event.surface else {
        return Err(unknown_region());
    };
    if region != MAKEFILE_REGION {
        return Err(unknown_region());
    }
    let words = Words::for_language(&event.language);
    Ok(vec![
        json!({ "type": "tool", "tool": MAKE_TOOL, "choose": true }),
        canvas(words, event.width),
    ])
}

fn unknown_region() -> PluginError {
    PluginError::new(
        "makefile.view.unknownRegion",
        "This view draws the Makefile plugin's settings region only.",
    )
}

struct Words {
    resolution_title: &'static str,
    resolution_label: &'static str,
    step_project: &'static str,
    step_project_detail: [&'static str; 2],
    step_global: &'static str,
    step_global_detail: [&'static str; 2],
    step_automatic: &'static str,
    step_automatic_detail: [&'static str; 2],
    discovery_title: &'static str,
    discovery_detail: [&'static str; 2],
    first: &'static str,
    then: &'static str,
    last: &'static str,
    used_for: &'static str,
    uses: [&'static str; 4],
}

impl Words {
    fn for_language(language: &str) -> &'static Self {
        if language.to_ascii_lowercase().starts_with("zh") {
            &ZH
        } else {
            &EN
        }
    }
}

const ZH: Words = Words {
    resolution_title: "Lithe 按这个顺序找 make",
    resolution_label: "Lithe 查找 make 程序与发现目标的顺序",
    step_project: "项目环境",
    step_project_detail: [
        "为项目固定的 make，其次项目选的版本",
        "在「工具链」页或上方选择器里设置",
    ],
    step_global: "全局默认",
    step_global_detail: [
        "全局固定的 make，其次默认版本",
        "在「工具链」页的全局范围选择",
    ],
    step_automatic: "自动发现",
    step_automatic_detail: [
        "PATH 中的 make / gmake",
        "→ Homebrew、系统与 MSYS2 安装",
    ],
    discovery_title: "目标发现方式",
    discovery_detail: [
        "优先用 make -p -R -r -q 解析完整规则",
        "make 不可用时自动退回静态解析器",
    ],
    first: "先找这里",
    then: "其次",
    last: "最后",
    used_for: "用于",
    uses: ["目标发现", "构建", "运行配置", "启动前任务"],
};

const EN: Words = Words {
    resolution_title: "How Lithe finds make",
    resolution_label: "Order in which Lithe finds make and discovers targets",
    step_project: "Project environment",
    step_project_detail: [
        "Pinned project make, then project choice",
        "Set on Toolchains or the picker above",
    ],
    step_global: "Global default",
    step_global_detail: [
        "Pinned global make, then global choice",
        "Set on Toolchains under Global default",
    ],
    step_automatic: "Automatic",
    step_automatic_detail: [
        "make / gmake on PATH",
        "-> Homebrew, system and MSYS2 installs",
    ],
    discovery_title: "Target discovery",
    discovery_detail: [
        "Uses make -p -R -r -q for full rules",
        "Falls back to static parser if absent",
    ],
    first: "Tried first",
    then: "Then",
    last: "Tried last",
    used_for: "Used for",
    uses: ["Targets", "Build", "Run", "Tasks"],
};

struct Step<'a> {
    title: &'a str,
    lines: [&'a str; 2],
    chip: &'a str,
    first: bool,
}

fn canvas(words: &Words, width: Option<f64>) -> Value {
    let scene_w = width.unwrap_or(DEFAULT_WIDTH).max(MIN_WIDTH);
    let step_w = scene_w - STEP_X;
    let steps = [
        Step {
            title: words.step_project,
            lines: words.step_project_detail,
            chip: words.first,
            first: true,
        },
        Step {
            title: words.step_global,
            lines: words.step_global_detail,
            chip: words.then,
            first: false,
        },
        Step {
            title: words.step_automatic,
            lines: words.step_automatic_detail,
            chip: words.last,
            first: false,
        },
    ];

    let mut nodes = vec![text(0.0, 17.0, words.resolution_title, "strong")];
    for (index, step) in steps.iter().enumerate() {
        let y = HEADER + index as f64 * (STEP_H + STEP_GAP);
        if index > 0 {
            let above = y - STEP_GAP - STEP_H + TITLE_MIDDLE + DOT / 2.0 + 4.0;
            let below = y + TITLE_MIDDLE - DOT / 2.0 - 4.0;
            nodes.push(
                path(format!("M{RAIL} {above} V{below}"))
                    .stroke(token("$line"), 2.0)
                    .set("cap", "round"),
            );
        }
        nodes.push(draw_step(y, index + 1, step, step_w));
    }
    nodes.push(draw_discovery(words, step_w));
    nodes.extend(draw_uses(words).0);

    let mut result = json!({
        "type": "canvas",
        "id": CANVAS_ID,
        "label": words.resolution_label,
        "height": HEIGHT.ceil() as u64,
        "viewBox": [0, 0, scene_w, HEIGHT],
        "nodes": nodes.into_iter().map(Value::from).collect::<Vec<_>>(),
    });
    if width.is_some() {
        result["on"] = json!(["resize"]);
    }
    result
}

fn draw_step(y: f64, number: usize, step: &Step, step_w: f64) -> Node {
    let frame = rect(0.0, 0.0, step_w, STEP_H)
        .radius(8.0)
        .fill(token("$surface"))
        .stroke(token("$line"), 1.0);
    let (fill, ink) = if step.first {
        (token("$selection"), token("$text"))
    } else {
        (token("$badge"), token("$textSecondary"))
    };
    let (_, chip_w) = pill(0.0, 0.0, step.chip, fill.clone(), ink.clone());
    let chip_x = step_w - 10.0 - chip_w;
    let (chip, _) = pill(chip_x, TITLE_MIDDLE - 10.0, step.chip, fill, ink);
    let mut card = vec![
        frame,
        text(LINE_X, TITLE_MIDDLE + 4.5, step.title, "strong").max_width(chip_x - LINE_X - 10.0),
        chip,
    ];
    card.extend(draw_lines(&step.lines, step_w));
    group(
        0.0,
        y,
        vec![
            rect(RAIL - DOT / 2.0, TITLE_MIDDLE - DOT / 2.0, DOT, DOT)
                .radius(DOT / 2.0)
                .fill(token("$badge")),
            text(RAIL, TITLE_MIDDLE + 4.0, &number.to_string(), "caption")
                .align("center")
                .color(token("$textSecondary")),
            group(STEP_X, 0.0, card),
        ],
    )
}

fn draw_discovery(words: &Words, step_w: f64) -> Node {
    let mut card = vec![
        rect(0.0, 0.0, step_w, STEP_H)
            .radius(8.0)
            .stroke(token("$line"), 1.0)
            .dash(&[4.0, 3.0]),
        text(LINE_X, TITLE_MIDDLE + 4.5, words.discovery_title, "strong")
            .max_width(step_w - 2.0 * LINE_X),
    ];
    card.extend(draw_lines(&words.discovery_detail, step_w));
    group(
        0.0,
        DISCOVERY_Y,
        vec![
            path(format!(
                "M{RAIL} {} V{TITLE_MIDDLE} H{}",
                TITLE_MIDDLE + DOT / 2.0 + 4.0 - STEP_H - STEP_GAP,
                STEP_X - 6.0
            ))
            .stroke(token("$line"), 2.0)
            .set("cap", "round"),
            path(format!(
                "M{} {} L{} {TITLE_MIDDLE} L{} {}",
                STEP_X - 9.0,
                TITLE_MIDDLE - 4.0,
                STEP_X - 4.0,
                STEP_X - 9.0,
                TITLE_MIDDLE + 4.0
            ))
            .stroke(token("$line"), 2.0)
            .set("cap", "round"),
            group(STEP_X, 0.0, card),
        ],
    )
}

fn draw_lines(lines: &[&str; 2], card_w: f64) -> Vec<Node> {
    lines
        .iter()
        .zip([40.0, 55.0])
        .map(|(line, y)| {
            text(LINE_X, y, line, "caption")
                .color(token("$textSecondary"))
                .max_width(card_w - 2.0 * LINE_X)
        })
        .collect()
}

fn draw_uses(words: &Words) -> (Vec<Node>, f64) {
    let mut nodes =
        vec![text(0.0, USES_Y + 14.0, words.used_for, "caption").color(token("$textMuted"))];
    let mut x = text_width(words.used_for, CAPTION) + 8.0;
    let mut end = x;
    for word in words.uses {
        let (chip, width) = pill(x, USES_Y, word, token("$badge"), token("$textSecondary"));
        nodes.push(chip);
        end = x + width;
        x = end + 6.0;
    }
    (nodes, end)
}

struct Node(Map<String, Value>);

impl Node {
    fn of(kind: &str) -> Self {
        let mut node = Map::new();
        node.insert("type".into(), kind.into());
        Self(node)
    }

    fn set(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.insert(key.into(), value.into());
        self
    }

    fn at(self, key: &str, value: f64) -> Self {
        self.set(key, round(value))
    }

    fn fill(self, color: impl Into<Value>) -> Self {
        self.set("fill", color)
    }

    fn stroke(self, color: impl Into<Value>, width: f64) -> Self {
        self.set("stroke", color).at("width", width)
    }

    fn radius(self, radius: f64) -> Self {
        self.at("radius", radius)
    }

    fn color(self, color: impl Into<Value>) -> Self {
        self.set("color", color)
    }

    fn max_width(self, width: f64) -> Self {
        self.at("maxWidth", width)
    }

    fn align(self, align: &str) -> Self {
        self.set("align", align)
    }

    fn dash(self, lengths: &[f64]) -> Self {
        let lengths: Vec<Value> = lengths.iter().map(|length| round(*length)).collect();
        self.set("dash", lengths)
    }
}

impl From<Node> for Value {
    fn from(node: Node) -> Self {
        Value::Object(node.0)
    }
}

fn round(value: f64) -> Value {
    let tenths = (value * 10.0).round();
    if tenths % 10.0 == 0.0 {
        json!((tenths / 10.0) as i64)
    } else {
        json!(tenths / 10.0)
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Node {
    Node::of("rect").at("x", x).at("y", y).at("w", w).at("h", h)
}

fn text(x: f64, y: f64, value: &str, style: &str) -> Node {
    Node::of("text")
        .at("x", x)
        .at("y", y)
        .set("text", value)
        .set("style", style)
}

fn path(d: String) -> Node {
    Node::of("path").set("d", d)
}

fn group(x: f64, y: f64, children: Vec<Node>) -> Node {
    let children: Vec<Value> = children.into_iter().map(Value::from).collect();
    Node::of("group")
        .at("x", x)
        .at("y", y)
        .set("children", children)
}

fn token(name: &str) -> Value {
    Value::from(name)
}

fn text_width(value: &str, size: f64) -> f64 {
    let ems: f64 = value
        .chars()
        .map(|character| match character {
            ' ' => 0.3,
            '\u{2E80}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}' => 1.0,
            'A'..='Z' | '0'..='9' | 'm' | 'w' => 0.66,
            'i' | 'j' | 'l' | '.' | ',' | ':' | ';' | '\'' | '|' | '!' => 0.3,
            _ => 0.55,
        })
        .sum();
    ems * size
}

fn pill(x: f64, y: f64, words: &str, fill: Value, ink: Value) -> (Node, f64) {
    let width = text_width(words, CAPTION) + 16.0;
    let node = group(
        x,
        y,
        vec![
            rect(0.0, 0.0, width, 20.0).radius(10.0).fill(fill),
            text(width / 2.0, 14.0, words, "caption")
                .align("center")
                .color(ink),
        ],
    );
    (node, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(language: &str, width: Option<f64>) -> ViewEvent {
        let mut raw = json!({
            "event": "open",
            "instance": "i1",
            "surface": { "settingRegionID": MAKEFILE_REGION },
            "language": language,
            "settings": { "effective": {}, "draft": {} },
        });
        if let Some(w) = width {
            raw["width"] = json!(w);
        }
        ViewEvent::parse(&raw).unwrap()
    }

    #[test]
    fn renders_tool_picker_and_responsive_canvas_in_both_languages() {
        for lang in ["en", "zh-Hans"] {
            let nodes = render(&event(lang, None)).unwrap();
            assert_eq!(nodes.len(), 2);
            assert_eq!(
                nodes[0],
                json!({ "type": "tool", "tool": "make", "choose": true })
            );
            assert_eq!(nodes[1]["type"], "canvas");
            assert_eq!(nodes[1]["id"], CANVAS_ID);
            assert!(nodes[1].get("on").is_none());

            let wide = render(&event(lang, Some(640.0))).unwrap();
            assert_eq!(wide[1]["viewBox"][2], json!(640.0));
            assert_eq!(wide[1]["on"], json!(["resize"]));
        }
    }

    #[test]
    fn uses_row_and_cards_fit_minimum_width() {
        let room = MIN_WIDTH - STEP_X - 2.0 * LINE_X;
        for lang in ["en", "zh-Hans"] {
            let words = Words::for_language(lang);
            let (_, end) = draw_uses(words);
            assert!(end <= MIN_WIDTH, "{lang}: uses end {end} > {MIN_WIDTH}");
            for line in [
                words.step_project_detail,
                words.step_global_detail,
                words.step_automatic_detail,
                words.discovery_detail,
            ]
            .concat()
            {
                let width = text_width(line, CAPTION);
                assert!(width <= room, "{lang}: {line} ({width} > {room})");
            }
        }
    }

    #[test]
    fn rejects_unknown_region() {
        let raw = json!({
            "event": "open",
            "instance": "i1",
            "surface": { "settingRegionID": "other" },
            "language": "en",
            "settings": { "effective": {}, "draft": {} },
        });
        let err = render(&ViewEvent::parse(&raw).unwrap()).unwrap_err();
        assert_eq!(err.code(), "makefile.view.unknownRegion");
    }
}
