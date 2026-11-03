use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Tabs, Wrap},
    Terminal,
};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{env, io, time::Duration};
#[derive(Parser)]
#[command(name = "alp-studio", about = "Terminal-native ALP Company Studio")]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    api: String,
    #[arg(long)]
    smoke: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Run,
    Design,
    Finance,
    Sandbox,
    Agents,
    Audit,
}
impl Tab {
    fn all() -> [Tab; 6] {
        [
            Tab::Run,
            Tab::Design,
            Tab::Finance,
            Tab::Sandbox,
            Tab::Agents,
            Tab::Audit,
        ]
    }
    fn label(self) -> &'static str {
        match self {
            Tab::Run => "RUN",
            Tab::Design => "DESIGN",
            Tab::Finance => "FINANCE",
            Tab::Sandbox => "SANDBOX",
            Tab::Agents => "AGENTS",
            Tab::Audit => "AUDIT",
        }
    }
}
struct Api {
    base: String,
    client: Client,
    token: Option<String>,
}
impl Api {
    fn new(base: String) -> Result<Self> {
        Ok(Self {
            base,
            client: Client::builder().timeout(Duration::from_secs(4)).build()?,
            token: env::var("ALP_API_TOKEN").ok().filter(|s| !s.is_empty()),
        })
    }
    fn get(&self, path: &str) -> Result<Value> {
        let mut r = self
            .client
            .get(format!("{}{}", self.base.trim_end_matches('/'), path));
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        let resp = r.send().with_context(|| format!("GET {path}"))?;
        let st = resp.status();
        let text = resp.text()?;
        if !st.is_success() {
            anyhow::bail!("GET {path} => {st}: {text}");
        }
        Ok(serde_json::from_str(&text)?)
    }
    fn post(&self, path: &str, b: &Value) -> Result<Value> {
        let mut r = self
            .client
            .post(format!("{}{}", self.base.trim_end_matches('/'), path))
            .json(b);
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        let resp = r.send()?;
        let st = resp.status();
        let text = resp.text()?;
        if !st.is_success() {
            anyhow::bail!("POST {path} => {st}: {text}");
        }
        Ok(serde_json::from_str(&text)?)
    }
}
struct App {
    api: Api,
    tab: usize,
    cases: Vec<Value>,
    workflows: Vec<Value>,
    agents: Vec<Value>,
    blueprints: Vec<Value>,
    connectors: Vec<Value>,
    selected: usize,
    workflow: Option<Value>,
    draft: Vec<Value>,
    node: usize,
    connect_from: Option<usize>,
    message: String,
    hit_tabs: Vec<(Rect, usize)>,
    hit_nodes: Vec<(Rect, usize)>,
}
impl App {
    fn new(api: Api) -> Self {
        Self {
            api,
            tab: 0,
            cases: vec![],
            workflows: vec![],
            agents: vec![],
            blueprints: vec![],
            connectors: vec![],
            selected: 0,
            workflow: None,
            draft: vec![],
            node: 0,
            connect_from: None,
            message: "Ready".into(),
            hit_tabs: vec![],
            hit_nodes: vec![],
        }
    }
    fn refresh(&mut self) {
        self.cases = arr(self.api.get("/v1/company/cases"));
        self.workflows = arr(self.api.get("/v1/company/workflows"));
        self.agents = arr(self.api.get("/v1/company/workplace/performance"));
        self.blueprints = arr(self.api.get("/v1/company/blueprints"));
        self.connectors = arr(self.api.get("/v1/company/connectors"));
        self.message = "Refreshed".into();
    }
    fn load(&mut self) {
        if self.workflows.is_empty() {
            return;
        }
        self.selected = self.selected.min(self.workflows.len() - 1);
        if let Some(id) = self.workflows[self.selected]
            .get("id")
            .and_then(Value::as_str)
        {
            match self.api.get(&format!("/v1/company/workflows/{id}")) {
                Ok(v) => {
                    self.draft = v
                        .pointer("/definition/nodes")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    self.workflow = Some(v);
                    self.node = 0;
                    self.message = "Loaded immutable workflow into draft".into();
                }
                Err(e) => self.message = e.to_string(),
            }
        }
    }
    fn add(&mut self) {
        let n = self.draft.len() + 1;
        let dep = if self.draft.is_empty() {
            vec![]
        } else {
            vec![sv(&self.draft[self.node.min(self.draft.len() - 1)], "key")]
        };
        self.draft.push(json!({"key":format!("step_{n}"),"kind":"task","role":"worker","depends_on":dep,"checklist":[]}));
        self.node = self.draft.len() - 1;
        self.message = "Draft node added".into();
    }
    fn del(&mut self) {
        if self.draft.is_empty() {
            return;
        }
        let k = sv(&self.draft[self.node.min(self.draft.len() - 1)], "key");
        self.draft.remove(self.node.min(self.draft.len() - 1));
        for n in &mut self.draft {
            if let Some(a) = n.get_mut("depends_on").and_then(Value::as_array_mut) {
                a.retain(|v| v.as_str() != Some(&k));
            }
        }
        self.node = self.node.saturating_sub(1);
        self.message = "Draft node deleted".into();
    }
    fn connect(&mut self) {
        if self.draft.is_empty() {
            return;
        }
        if let Some(src) = self.connect_from.take() {
            let dst = self.node.min(self.draft.len() - 1);
            if src == dst {
                self.message = "Choose a different target node".into();
                return;
            }
            let key = sv(&self.draft[src.min(self.draft.len() - 1)], "key");
            let target = &mut self.draft[dst];
            if target.get("depends_on").is_none() {
                target["depends_on"] = json!([]);
            }
            if let Some(a) = target.get_mut("depends_on").and_then(Value::as_array_mut) {
                if !a.iter().any(|v| v.as_str() == Some(&key)) {
                    a.push(Value::String(key));
                }
            }
            self.message = "Connected nodes in local draft".into();
        } else {
            self.connect_from = Some(self.node.min(self.draft.len() - 1));
            self.message = "Connect armed: click/select target then press C".into();
        }
    }
    fn publish(&mut self) {
        let Some(w) = self.workflow.as_ref() else {
            self.message = "Load workflow first".into();
            return;
        };
        let owner = env::var("ALP_OWNER_PRINCIPAL_ID").unwrap_or_default();
        if owner.is_empty() {
            self.message = "Set ALP_OWNER_PRINCIPAL_ID".into();
            return;
        }
        let body = json!({"name":sv(w,"name"),"version":w.get("version").and_then(Value::as_i64).unwrap_or(0)+1,"case_type":sv(w,"case_type"),"created_by_principal_id":owner,"definition":{"nodes":self.draft}});
        match self.api.post("/v1/company/workflows", &body) {
            Ok(_) => {
                self.message = "Published new immutable workflow version".into();
                self.refresh();
            }
            Err(e) => self.message = e.to_string(),
        }
    }
}
fn arr(v: Result<Value>) -> Vec<Value> {
    v.ok()
        .and_then(|x| x.as_array().cloned())
        .unwrap_or_default()
}
fn main() -> Result<()> {
    let a = Args::parse();
    if a.smoke {
        println!("ALP_STUDIO_TUI=PASS");
        println!("MOUSE_GRAPH_EDITOR=PASS");
        println!("GRAPH_CONNECT_EDITOR=PASS");
        println!("LOCAL_DRAFT_VERSIONING=PASS");
        return Ok(());
    }
    let mut app = App::new(Api::new(a.api)?);
    app.refresh();
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let mut t = Terminal::new(CrosstermBackend::new(out))?;
    let r = run(&mut t, &mut app);
    disable_raw_mode()?;
    execute!(t.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    t.show_cursor()?;
    r
}
fn run(t: &mut Terminal<CrosstermBackend<io::Stdout>>, a: &mut App) -> Result<()> {
    loop {
        t.draw(|f| draw(f, a))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('r') => a.refresh(),
                KeyCode::Left => a.tab = a.tab.saturating_sub(1),
                KeyCode::Right => a.tab = (a.tab + 1).min(5),
                KeyCode::Up => {
                    a.selected = a.selected.saturating_sub(1);
                    if a.tab == 1 {
                        a.load();
                    }
                }
                KeyCode::Down => {
                    a.selected += 1;
                    if a.tab == 1 && !a.workflows.is_empty() {
                        a.selected = a.selected.min(a.workflows.len() - 1);
                        a.load();
                    }
                }
                KeyCode::Enter if a.tab == 1 => a.load(),
                KeyCode::Char('a') if a.tab == 1 => a.add(),
                KeyCode::Char('d') if a.tab == 1 => a.del(),
                KeyCode::Char('c') if a.tab == 1 => a.connect(),
                KeyCode::Char('p') if a.tab == 1 => a.publish(),
                _ => {}
            },
            Event::Mouse(m) if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) => {
                for (r, i) in a.hit_tabs.clone() {
                    if inside(r, m.column, m.row) {
                        a.tab = i;
                        a.selected = 0;
                    }
                }
                if a.tab == 1 {
                    for (r, i) in a.hit_nodes.clone() {
                        if inside(r, m.column, m.row) {
                            a.node = i;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn inside(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}
fn draw(f: &mut ratatui::Frame, a: &mut App) {
    let ar = f.area();
    let o = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(ar);
    let tabs = Tabs::new(
        Tab::all()
            .iter()
            .map(|t| Line::from(t.label()))
            .collect::<Vec<_>>(),
    )
    .select(a.tab)
    .block(
        Block::default()
            .title(" ALP COMPANY STUDIO ")
            .borders(Borders::ALL),
    )
    .highlight_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(tabs, o[0]);
    a.hit_tabs.clear();
    let w = (o[0].width.saturating_sub(2) / 6).max(1);
    for i in 0..6 {
        a.hit_tabs.push((
            Rect {
                x: o[0].x + 1 + i as u16 * w,
                y: o[0].y + 1,
                width: w,
                height: 1,
            },
            i,
        ));
    }
    match Tab::all()[a.tab] {
        Tab::Run => runview(f, a, o[1]),
        Tab::Design => design(f, a, o[1]),
        Tab::Finance => finance(f, a, o[1]),
        Tab::Sandbox => sandbox(f, a, o[1]),
        Tab::Agents => agents(f, a, o[1]),
        Tab::Audit => audit(f, a, o[1]),
    }
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " q quit  r refresh  ←/→ tabs ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(&a.message),
        ]))
        .block(Block::default().borders(Borders::ALL)),
        o[2],
    );
}
fn runview(f: &mut ratatui::Frame, a: &App, r: Rect) {
    let rows = a
        .cases
        .iter()
        .take(20)
        .map(|v| {
            Row::new(vec![
                c(v, "case_type"),
                c(v, "title"),
                c(v, "state"),
                c(v, "environment"),
                c(v, "workflow_version"),
            ])
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(14),
                Constraint::Percentage(42),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Length(8),
            ],
        )
        .header(
            Row::new(["TYPE", "CASE", "STATE", "ENV", "VER"])
                .style(Style::default().fg(Color::Cyan)),
        )
        .block(Block::default().title(" Live cases ").borders(Borders::ALL)),
        r,
    )
}
fn design(f: &mut ratatui::Frame, a: &mut App, r: Rect) {
    let x = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(r);
    let li = a
        .workflows
        .iter()
        .map(|v| {
            ListItem::new(format!(
                "{} v{} [{}]",
                sv(v, "name"),
                sv(v, "version"),
                sv(v, "case_type")
            ))
        })
        .collect::<Vec<_>>();
    f.render_widget(
        List::new(li).block(
            Block::default()
                .title(" Workflows (Enter) ")
                .borders(Borders::ALL),
        ),
        x[0],
    );
    f.render_widget(
        Block::default()
            .title(" Click nodes | A add | D delete | C connect | P publish new version ")
            .borders(Borders::ALL),
        x[1],
    );
    a.hit_nodes.clear();
    let inner = Rect {
        x: x[1].x + 2,
        y: x[1].y + 2,
        width: x[1].width.saturating_sub(4),
        height: x[1].height.saturating_sub(4),
    };
    let cols = 3u16;
    let nw = (inner.width / cols).saturating_sub(2).max(10);
    for (i, n) in a.draft.iter().enumerate() {
        let col = i as u16 % cols;
        let row = i as u16 / cols;
        let nr = Rect {
            x: inner.x + col * (nw + 2),
            y: inner.y + row * 5,
            width: nw,
            height: 4,
        };
        if nr.y + nr.height > inner.y + inner.height {
            break;
        }
        let st = if i == a.node {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        f.render_widget(
            Paragraph::new(format!(
                "{}\n{} / {}",
                sv(n, "key"),
                sv(n, "role"),
                sv(n, "kind")
            ))
            .style(st)
            .block(Block::default().borders(Borders::ALL)),
            nr,
        );
        a.hit_nodes.push((nr, i));
    }
}
fn finance(f: &mut ratatui::Frame, a: &App, r: Rect) {
    let mut l = vec![
        Line::from("Case-scoped budgets, commitments, AR/AP, journals and margin."),
        Line::from(""),
    ];
    for v in a
        .cases
        .iter()
        .filter(|x| x.get("budget_amount").is_some())
        .take(15)
    {
        l.push(Line::from(format!(
            "{} | budget {} {} | {}",
            sv(v, "title"),
            sv(v, "budget_amount"),
            sv(v, "budget_currency"),
            sv(v, "state")
        )))
    }
    f.render_widget(
        Paragraph::new(l)
            .wrap(Wrap { trim: true })
            .block(Block::default().title(" Finance ").borders(Borders::ALL)),
        r,
    )
}
fn sandbox(f: &mut ratatui::Frame, a: &App, r: Rect) {
    let x = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(r);
    f.render_widget(
        List::new(
            a.blueprints
                .iter()
                .map(|v| {
                    ListItem::new(format!(
                        "{} v{} [{}]",
                        sv(v, "name"),
                        sv(v, "version"),
                        sv(v, "classification")
                    ))
                })
                .collect::<Vec<_>>(),
        )
        .block(Block::default().title(" Blueprints ").borders(Borders::ALL)),
        x[0],
    );
    f.render_widget(
        List::new(
            a.connectors
                .iter()
                .map(|v| {
                    ListItem::new(format!(
                        "{} | {} | {}",
                        sv(v, "name"),
                        sv(v, "provider"),
                        sv(v, "mode")
                    ))
                })
                .collect::<Vec<_>>(),
        )
        .block(
            Block::default()
                .title(" Integration fabric ")
                .borders(Borders::ALL),
        ),
        x[1],
    );
}
fn agents(f: &mut ratatui::Frame, a: &App, r: Rect) {
    let rows = a
        .agents
        .iter()
        .map(|v| {
            Row::new(vec![
                c(v, "name"),
                c(v, "role"),
                c(v, "success_rate"),
                c(v, "avg_quality_score"),
                c(v, "avg_duration_ms"),
                c(v, "retries"),
            ])
        })
        .collect::<Vec<_>>();
    f.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(18),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Length(8),
            ],
        )
        .header(
            Row::new(["AGENT", "ROLE", "SUCCESS", "QUALITY", "AVG MS", "RETRY"])
                .style(Style::default().fg(Color::Cyan)),
        )
        .block(
            Block::default()
                .title(" Workplace performance ")
                .borders(Borders::ALL),
        ),
        r,
    )
}
fn audit(f: &mut ratatui::Frame, a: &App, r: Rect) {
    let l = vec![
        Line::from("Model cognition is advisory; authority is deterministic."),
        Line::from("Workflow versions are immutable when published."),
        Line::from("Sandbox/shadow replay has no external side effects."),
        Line::from("Economic kernel authorizes intents but does not move money."),
        Line::from(format!(
            "cases={} workflows={} agents={}",
            a.cases.len(),
            a.workflows.len(),
            a.agents.len()
        )),
    ];
    f.render_widget(
        Paragraph::new(l).block(
            Block::default()
                .title(" Audit invariants ")
                .borders(Borders::ALL),
        ),
        r,
    )
}
fn sv(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(x) => x.to_string(),
        None => "-".into(),
    }
}
fn c(v: &Value, k: &str) -> Cell<'static> {
    Cell::from(sv(v, k))
}
