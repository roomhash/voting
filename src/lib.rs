use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

const STATE_SCHEMA: &str = "roomhash.voting/state-v2";
const LOCAL_NOTICE: &str =
    "不同节点看到的统计结果可能不同，取决于各自实际收集到的数据；这是本地可见视图，不是全局强一致结果。";
const MAX_EVENTS: usize = 10_000;
const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_POLL_DURATION_MS: u64 = 14 * DAY_MS;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitContext {
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub peer_id: String,
    pub identity_seed: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub instance_id: String,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub saved_state: Option<Value>,
    #[serde(default)]
    pub now_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OptionDef {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum PublicEvent {
    PollCreated {
        event_id: String,
        creator_hash: String,
        nick: String,
        title: String,
        options: Vec<OptionDef>,
        created_at_ms: u64,
        expires_at_ms: u64,
    },
    Ballot {
        event_id: String,
        poll_id: String,
        expires_at_ms: u64,
        voter_hash: String,
        nick: String,
        option_id: String,
        revision: u64,
    },
    PollDeleted {
        event_id: String,
        poll_id: String,
        expires_at_ms: u64,
        creator_hash: String,
    },
}

impl PublicEvent {
    fn id(&self) -> &str {
        match self {
            Self::PollCreated { event_id, .. }
            | Self::Ballot { event_id, .. }
            | Self::PollDeleted { event_id, .. } => event_id,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicState {
    #[serde(default = "state_schema")]
    pub schema: String,
    #[serde(default)]
    pub events: Vec<PublicEvent>,
}

fn state_schema() -> String {
    STATE_SCHEMA.into()
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x <= self.x + self.width && y <= self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Results,
    Create,
    Audit,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Results => "投票",
            Self::Create => "创建",
            Self::Audit => "公开票据",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Layout {
    width: f64,
    height: f64,
    header: Rect,
    navigation: Rect,
    content: Rect,
    fullscreen: Rect,
    wide: bool,
}

pub struct VotingApp {
    nickname: String,
    voter_hash: String,
    viewport_width: f64,
    viewport_height: f64,
    fullscreen: bool,
    tab: Tab,
    scroll: f64,
    now_ms: u64,
    draft_title: String,
    draft_options: String,
    draft_duration_days: u8,
    selected_poll_id: Option<String>,
    selected_option: Option<String>,
    focused: String,
    notice: String,
    events: BTreeMap<String, PublicEvent>,
    emitted: Vec<PublicEvent>,
    effects: Vec<Value>,
}

impl VotingApp {
    pub fn new(context: InitContext) -> Result<Self, String> {
        if !valid_seed(&context.identity_seed) {
            return Err("identitySeed 必须是 64 位十六进制字符串".into());
        }
        let mut app = Self {
            nickname: clean(&context.nickname, 80).unwrap_or_else(|| "Anonymous".into()),
            voter_hash: sha256_hex(context.identity_seed.as_bytes()),
            viewport_width: 960.0,
            viewport_height: 640.0,
            fullscreen: false,
            tab: Tab::Create,
            scroll: 0.0,
            now_ms: context.now_ms,
            draft_title: String::new(),
            draft_options: String::new(),
            draft_duration_days: 7,
            selected_poll_id: None,
            selected_option: None,
            focused: String::new(),
            notice: LOCAL_NOTICE.into(),
            events: BTreeMap::new(),
            emitted: Vec::new(),
            effects: Vec::new(),
        };
        if let Some(saved) = context.saved_state {
            app.merge_snapshot_value(&saved);
        }
        app.purge_retired();
        if !app.active_polls().is_empty() {
            app.tab = Tab::Results;
        }
        Ok(app)
    }

    pub fn voter_hash(&self) -> &str {
        &self.voter_hash
    }

    fn layout(&self) -> Layout {
        let width = self.viewport_width.clamp(320.0, 4096.0);
        let height = self.viewport_height.clamp(480.0, 2160.0);
        // Preserve a generous content column in embedded hosts. Move to the
        // permanent sidebar only when the viewport is genuinely desktop-wide.
        let wide = width >= 1024.0;
        let header = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: 76.0,
        };
        let fullscreen = Rect {
            x: width - 88.0,
            y: 14.0,
            width: 72.0,
            height: 48.0,
        };
        let (navigation, content) = if wide {
            (
                Rect {
                    x: 16.0,
                    y: 92.0,
                    width: 220.0,
                    height: height - 108.0,
                },
                Rect {
                    x: 252.0,
                    y: 92.0,
                    width: width - 268.0,
                    height: height - 108.0,
                },
            )
        } else {
            (
                Rect {
                    x: 12.0,
                    y: 84.0,
                    width: width - 24.0,
                    height: 72.0,
                },
                Rect {
                    x: 12.0,
                    y: 168.0,
                    width: width - 24.0,
                    height: height - 180.0,
                },
            )
        };
        Layout {
            width,
            height,
            header,
            navigation,
            content,
            fullscreen,
            wide,
        }
    }

    pub fn dispatch(&mut self, input: Value) -> Result<Value, String> {
        self.emitted.clear();
        self.effects.clear();
        if let Some(now_ms) = input
            .get("nowMs")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
        {
            self.now_ms = now_ms;
        }
        self.purge_retired();
        match input.get("kind").and_then(Value::as_str).unwrap_or("") {
            "viewport" => {
                self.viewport_width = input.get("width").and_then(Value::as_f64).unwrap_or(960.0);
                self.viewport_height = input.get("height").and_then(Value::as_f64).unwrap_or(640.0);
                self.fullscreen = input
                    .get("fullscreen")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                self.clamp_scroll();
            }
            "pointer" => self.handle_pointer(&input),
            "wheel" => {
                let delta = input.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0);
                self.scroll = (self.scroll + delta).max(0.0);
                self.clamp_scroll();
            }
            "key" => self.handle_key(&input),
            "text" => self.handle_text(&input),
            "remote" => {
                let had_poll = !self.active_polls().is_empty();
                let event: PublicEvent =
                    serde_json::from_value(input.get("event").cloned().ok_or("缺少 remote event")?)
                        .map_err(|_| "remote event 格式错误")?;
                if self.insert_verified(event, false)
                    && !had_poll
                    && !self.active_polls().is_empty()
                {
                    self.tab = Tab::Results;
                    self.notice = "收到一个共享投票".into();
                }
            }
            "state-request" => return Ok(self.output(true)),
            "snapshot" => {
                let count = self.merge_snapshot_value(input.get("state").ok_or("缺少 state")?);
                if count > 0 && !self.active_polls().is_empty() {
                    self.notice = format!("已合并 {count} 条公开事件");
                }
            }
            "host-result" => {}
            _ => return Err("未知 dispatch kind".into()),
        }
        self.purge_retired();
        Ok(self.output(false))
    }

    fn handle_text(&mut self, input: &Value) {
        let request = input.get("requestId").and_then(Value::as_str).unwrap_or("");
        let value = input.get("value").and_then(Value::as_str).unwrap_or("");
        match request {
            "poll-title" => self.draft_title = value.chars().take(200).collect(),
            "poll-options" => self.draft_options = value.chars().take(4_000).collect(),
            _ => return,
        }
        self.focused = request.into();
    }

    fn handle_key(&mut self, input: &Value) {
        if input.get("phase").and_then(Value::as_str) != Some("down") {
            return;
        }
        match input.get("key").and_then(Value::as_str).unwrap_or("") {
            "ArrowLeft" => {
                self.tab = match self.tab {
                    Tab::Results => Tab::Audit,
                    Tab::Create => Tab::Results,
                    Tab::Audit => Tab::Create,
                };
                self.scroll = 0.0;
            }
            "ArrowRight" => {
                self.tab = match self.tab {
                    Tab::Results => Tab::Create,
                    Tab::Create => Tab::Audit,
                    Tab::Audit => Tab::Results,
                };
                self.scroll = 0.0;
            }
            "Enter" if self.tab == Tab::Results => self.submit_vote(),
            "Enter" if self.tab == Tab::Create && self.focused != "poll-options" => {
                self.submit_create()
            }
            _ => {}
        }
    }

    fn handle_pointer(&mut self, input: &Value) {
        if input.get("phase").and_then(Value::as_str) != Some("down") {
            return;
        }
        let x = input.get("x").and_then(Value::as_f64).unwrap_or(0.0);
        let y = input.get("y").and_then(Value::as_f64).unwrap_or(0.0);
        let layout = self.layout();
        if layout.fullscreen.contains(x, y) {
            self.effects.push(json!({
                "type":"fullscreen",
                "requestId":"fullscreen",
                "enabled":!self.fullscreen
            }));
            return;
        }
        for (tab, rect) in self.tab_rects(layout) {
            if rect.contains(x, y) {
                self.tab = tab;
                self.scroll = 0.0;
                return;
            }
        }
        match self.tab {
            Tab::Create => self.handle_create_pointer(layout, x, y),
            Tab::Results => self.handle_results_pointer(layout, x, y),
            Tab::Audit => {}
        }
    }

    fn handle_create_pointer(&mut self, layout: Layout, x: f64, y: f64) {
        let (panel, title, options, duration, submit) = self.create_rects(layout);
        if !panel.contains(x, y) {
            return;
        }
        if title.contains(x, y) {
            self.focused = "poll-title".into();
            self.effects.push(json!({
                "type":"text-input","requestId":"poll-title","value":self.draft_title,
                "inputMode":"text","multiline":false,"label":"投票标题"
            }));
        } else if options.contains(x, y) {
            self.focused = "poll-options".into();
            self.effects.push(json!({
                "type":"text-input","requestId":"poll-options","value":self.draft_options,
                "inputMode":"text","multiline":true,"label":"投票选项，每行一项"
            }));
        } else if duration.contains(x, y) {
            self.draft_duration_days = match self.draft_duration_days {
                1 => 3,
                3 => 7,
                7 => 14,
                _ => 1,
            };
            self.notice = format!("有效期：{} 天", self.draft_duration_days);
        } else if submit.contains(x, y) {
            self.submit_create();
        }
    }

    fn handle_results_pointer(&mut self, layout: Layout, x: f64, y: f64) {
        if self.selected_poll().is_none() {
            for (poll, rect) in self.poll_list_rects(layout) {
                if rect.contains(x, y) && layout.content.contains(x, y) {
                    self.selected_poll_id = Some(poll.id().into());
                    self.selected_option = None;
                    self.scroll = 0.0;
                    return;
                }
            }
            return;
        }
        let (back, delete) = self.detail_action_rects(layout);
        if back.contains(x, y) {
            self.selected_poll_id = None;
            self.selected_option = None;
            self.scroll = 0.0;
            return;
        }
        if delete.contains(x, y) && self.can_delete_selected() {
            self.delete_selected_poll();
            return;
        }
        let Some(PublicEvent::PollCreated { options, .. }) = self.selected_poll() else {
            return;
        };
        for (option, rect) in self.option_rects(layout, &options) {
            if rect.contains(x, y) && layout.content.contains(x, y) {
                self.selected_option = Some(option.id);
                self.notice = "已选择，提交后会公开广播".into();
                return;
            }
        }
        let vote = self.vote_button(layout);
        if vote.contains(x, y) {
            self.submit_vote();
        }
    }

    fn submit_create(&mut self) {
        match self.create_poll(&self.draft_title, &self.draft_options) {
            Ok(event) => {
                let poll_id = event.id().to_string();
                self.insert_verified(event, true);
                self.draft_title.clear();
                self.draft_options.clear();
                self.selected_option = None;
                self.selected_poll_id = Some(poll_id);
                self.focused.clear();
                self.tab = Tab::Results;
                self.scroll = 0.0;
                self.announce("投票已创建并广播");
            }
            Err(error) => self.announce(&error),
        }
    }

    fn submit_vote(&mut self) {
        let Some(option_id) = self.selected_option.clone() else {
            self.announce("请先选择一个选项");
            return;
        };
        match self.cast_vote(&option_id) {
            Ok(event) => {
                self.insert_verified(event, true);
                self.announce("选票已公开广播；同一用户 Hash 仅计最新 revision");
            }
            Err(error) => self.announce(&error),
        }
    }

    fn announce(&mut self, message: &str) {
        self.notice = message.chars().take(240).collect();
        self.effects.push(json!({
            "type":"announce","tone":"polite","text":self.notice
        }));
    }

    fn create_poll(&self, raw_title: &str, raw_options: &str) -> Result<PublicEvent, String> {
        let title = clean(raw_title, 200).ok_or("投票标题不能为空")?;
        if self.now_ms == 0 {
            return Err("当前时间不可用，请稍后重试".into());
        }
        let mut seen_labels = BTreeSet::new();
        let labels: Vec<String> = raw_options
            .lines()
            .filter_map(|line| clean(line, 120))
            .filter(|label| seen_labels.insert(label.clone()))
            .collect();
        if labels.len() < 2 || labels.len() > 32 {
            return Err("选项必须为 2 至 32 个非重复行".into());
        }
        let options: Vec<OptionDef> = labels
            .into_iter()
            .enumerate()
            .map(|(index, label)| OptionDef {
                id: sha256_hex(format!("option\0{index}\0{label}").as_bytes())[..16].into(),
                label,
            })
            .collect();
        let canonical_options = canonical_options(&options);
        let duration_ms = u64::from(self.draft_duration_days.clamp(1, 14)) * DAY_MS;
        let expires_at_ms = self.now_ms.saturating_add(duration_ms);
        let event_id = sha256_hex(
            format!(
                "poll-created\0{}\0{}\0{}\0{}\0{}\0{}",
                self.voter_hash,
                self.nickname,
                title,
                canonical_options,
                self.now_ms,
                expires_at_ms
            )
            .as_bytes(),
        );
        Ok(PublicEvent::PollCreated {
            event_id,
            creator_hash: self.voter_hash.clone(),
            nick: self.nickname.clone(),
            title,
            options,
            created_at_ms: self.now_ms,
            expires_at_ms,
        })
    }

    fn cast_vote(&self, option_id: &str) -> Result<PublicEvent, String> {
        let Some(PublicEvent::PollCreated {
            event_id: poll_id,
            options,
            expires_at_ms,
            ..
        }) = self.selected_poll()
        else {
            return Err("尚未创建投票".into());
        };
        if !options.iter().any(|option| option.id == option_id) {
            return Err("无效选项".into());
        }
        let revision = self
            .winning_ballots()
            .get(&self.voter_hash)
            .and_then(|event| match event {
                PublicEvent::Ballot { revision, .. } => revision.checked_add(1),
                _ => None,
            })
            .unwrap_or(1);
        let event_id = sha256_hex(
            format!(
                "ballot\0{}\0{}\0{}\0{}\0{}\0{}",
                poll_id, expires_at_ms, self.voter_hash, self.nickname, option_id, revision
            )
            .as_bytes(),
        );
        Ok(PublicEvent::Ballot {
            event_id,
            poll_id,
            expires_at_ms,
            voter_hash: self.voter_hash.clone(),
            nick: self.nickname.clone(),
            option_id: option_id.into(),
            revision,
        })
    }

    fn insert_verified(&mut self, event: PublicEvent, emit: bool) -> bool {
        if !verify_event(&event)
            || self.events.contains_key(event.id())
            || self.events.len() >= MAX_EVENTS
        {
            return false;
        }
        match &event {
            PublicEvent::PollCreated {
                created_at_ms,
                expires_at_ms,
                ..
            } if self.now_ms > 0
                && (*expires_at_ms <= self.now_ms
                    || *created_at_ms > self.now_ms.saturating_add(5 * 60 * 1_000)) =>
            {
                return false;
            }
            PublicEvent::Ballot {
                poll_id,
                expires_at_ms,
                ..
            }
            | PublicEvent::PollDeleted {
                poll_id,
                expires_at_ms,
                ..
            } => {
                if self.now_ms > 0
                    && (*expires_at_ms <= self.now_ms
                        || *expires_at_ms
                            > self
                                .now_ms
                                .saturating_add(MAX_POLL_DURATION_MS + 5 * 60 * 1_000))
                {
                    return false;
                }
                if self.events.values().any(|candidate| {
                    matches!(candidate,
                        PublicEvent::PollCreated {
                            event_id,
                            expires_at_ms: poll_expiry,
                            ..
                        } if event_id == poll_id && poll_expiry != expires_at_ms)
                }) {
                    return false;
                }
            }
            _ => {}
        }
        let created_poll = match &event {
            PublicEvent::PollCreated {
                event_id,
                expires_at_ms,
                ..
            } => Some((event_id.clone(), *expires_at_ms)),
            _ => None,
        };
        if emit {
            self.emitted.push(event.clone());
        }
        self.events.insert(event.id().into(), event);
        if let Some((poll_id, poll_expiry)) = created_poll {
            self.events.retain(|_, candidate| match candidate {
                PublicEvent::Ballot {
                    poll_id: candidate_poll,
                    expires_at_ms,
                    ..
                }
                | PublicEvent::PollDeleted {
                    poll_id: candidate_poll,
                    expires_at_ms,
                    ..
                } if candidate_poll == &poll_id => *expires_at_ms == poll_expiry,
                _ => true,
            });
        }
        true
    }

    fn merge_snapshot_value(&mut self, value: &Value) -> usize {
        let Ok(snapshot) = serde_json::from_value::<PublicState>(value.clone()) else {
            return 0;
        };
        if snapshot.schema != STATE_SCHEMA {
            return 0;
        }
        let mut events = snapshot.events;
        events.sort_by_key(|event| match event {
            PublicEvent::PollCreated { .. } => 0,
            PublicEvent::PollDeleted { .. } => 1,
            PublicEvent::Ballot { .. } => 2,
        });
        let count = events
            .into_iter()
            .filter(|event| self.insert_verified(event.clone(), false))
            .count();
        self.purge_retired();
        count
    }

    pub fn public_state(&self) -> PublicState {
        PublicState {
            schema: STATE_SCHEMA.into(),
            events: self.events.values().cloned().collect(),
        }
    }

    fn deleted_poll_ids(&self) -> BTreeSet<String> {
        let creators: BTreeMap<String, String> = self
            .events
            .values()
            .filter_map(|event| match event {
                PublicEvent::PollCreated {
                    event_id,
                    creator_hash,
                    ..
                } => Some((event_id.clone(), creator_hash.clone())),
                _ => None,
            })
            .collect();
        self.events
            .values()
            .filter_map(|event| match event {
                PublicEvent::PollDeleted {
                    poll_id,
                    creator_hash,
                    ..
                } if creators.get(poll_id) == Some(creator_hash) => Some(poll_id.clone()),
                _ => None,
            })
            .collect()
    }

    fn active_polls(&self) -> Vec<PublicEvent> {
        let deleted = self.deleted_poll_ids();
        let mut polls: Vec<PublicEvent> = self
            .events
            .values()
            .filter_map(|event| match event {
                PublicEvent::PollCreated {
                    event_id,
                    expires_at_ms,
                    ..
                } if !deleted.contains(event_id)
                    && (self.now_ms == 0 || *expires_at_ms > self.now_ms) =>
                {
                    Some(event.clone())
                }
                _ => None,
            })
            .collect();
        polls.sort_by(|left, right| match (left, right) {
            (
                PublicEvent::PollCreated {
                    created_at_ms: left_created,
                    event_id: left_id,
                    ..
                },
                PublicEvent::PollCreated {
                    created_at_ms: right_created,
                    event_id: right_id,
                    ..
                },
            ) => right_created
                .cmp(left_created)
                .then_with(|| right_id.cmp(left_id)),
            _ => std::cmp::Ordering::Equal,
        });
        polls
    }

    fn selected_poll(&self) -> Option<PublicEvent> {
        let selected = self.selected_poll_id.as_deref()?;
        self.active_polls()
            .into_iter()
            .find(|poll| poll.id() == selected)
    }

    fn purge_retired(&mut self) {
        let expired: BTreeSet<String> = self
            .events
            .values()
            .filter_map(|event| match event {
                PublicEvent::PollCreated {
                    event_id,
                    expires_at_ms,
                    ..
                } if self.now_ms > 0 && *expires_at_ms <= self.now_ms => Some(event_id.clone()),
                _ => None,
            })
            .collect();
        let deleted = self.deleted_poll_ids();
        let retired: BTreeSet<String> = expired.union(&deleted).cloned().collect();
        self.events.retain(|_, event| match event {
            PublicEvent::PollCreated { event_id, .. } => !retired.contains(event_id),
            PublicEvent::Ballot {
                poll_id,
                expires_at_ms,
                ..
            } => !retired.contains(poll_id) && (self.now_ms == 0 || *expires_at_ms > self.now_ms),
            PublicEvent::PollDeleted {
                poll_id,
                expires_at_ms,
                ..
            } => !expired.contains(poll_id) && (self.now_ms == 0 || *expires_at_ms > self.now_ms),
        });
        if self
            .selected_poll_id
            .as_ref()
            .is_some_and(|poll_id| retired.contains(poll_id))
        {
            self.selected_poll_id = None;
            self.selected_option = None;
            self.scroll = 0.0;
        }
    }

    fn can_delete_selected(&self) -> bool {
        matches!(self.selected_poll(), Some(PublicEvent::PollCreated { creator_hash, .. }) if creator_hash == self.voter_hash)
    }

    fn delete_selected_poll(&mut self) {
        let Some(PublicEvent::PollCreated {
            event_id: poll_id,
            creator_hash,
            expires_at_ms,
            ..
        }) = self.selected_poll()
        else {
            return;
        };
        if creator_hash != self.voter_hash {
            self.announce("只有创建者可以删除这个投票");
            return;
        }
        let event_id = sha256_hex(
            format!(
                "poll-deleted\0{}\0{}\0{}",
                poll_id, expires_at_ms, self.voter_hash
            )
            .as_bytes(),
        );
        self.insert_verified(
            PublicEvent::PollDeleted {
                event_id,
                poll_id,
                expires_at_ms,
                creator_hash: self.voter_hash.clone(),
            },
            true,
        );
        self.purge_retired();
        self.announce("投票已删除并广播");
    }

    fn winning_ballots_for(
        &self,
        poll_id: &str,
        options: &[OptionDef],
    ) -> BTreeMap<String, PublicEvent> {
        let mut winners: BTreeMap<String, PublicEvent> = BTreeMap::new();
        for event in self.events.values() {
            let PublicEvent::Ballot {
                poll_id: candidate_poll,
                voter_hash,
                revision,
                event_id,
                option_id,
                ..
            } = event
            else {
                continue;
            };
            if candidate_poll != poll_id || !options.iter().any(|item| item.id == *option_id) {
                continue;
            }
            let wins = match winners.get(voter_hash) {
                Some(PublicEvent::Ballot {
                    revision: old_revision,
                    event_id: old_id,
                    ..
                }) => (*revision, event_id) > (*old_revision, old_id),
                _ => true,
            };
            if wins {
                winners.insert(voter_hash.clone(), event.clone());
            }
        }
        winners
    }

    pub fn winning_ballots(&self) -> BTreeMap<String, PublicEvent> {
        let Some(PublicEvent::PollCreated {
            event_id: poll_id,
            options,
            ..
        }) = self.selected_poll()
        else {
            return BTreeMap::new();
        };
        self.winning_ballots_for(&poll_id, &options)
    }

    fn option_counts(&self, options: &[OptionDef]) -> BTreeMap<String, usize> {
        let mut counts: BTreeMap<String, usize> =
            options.iter().map(|item| (item.id.clone(), 0)).collect();
        for event in self.winning_ballots().values() {
            if let PublicEvent::Ballot { option_id, .. } = event {
                if let Some(count) = counts.get_mut(option_id) {
                    *count += 1;
                }
            }
        }
        counts
    }

    fn max_scroll(&self) -> f64 {
        match self.tab {
            Tab::Results => self
                .selected_poll()
                .and_then(|event| match event {
                    PublicEvent::PollCreated { options, .. } => Some(options.len()),
                    _ => None,
                })
                .map(|count| (count as f64 * 72.0 - self.layout().content.height + 360.0).max(0.0))
                .unwrap_or_else(|| {
                    (self.active_polls().len() as f64 * 86.0 - self.layout().content.height + 112.0)
                        .max(0.0)
                }),
            Tab::Audit => (self.audit_rows().len() as f64 * 72.0 - self.layout().content.height
                + 150.0)
                .max(0.0),
            Tab::Create => (472.0 - self.layout().content.height).max(0.0),
        }
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn tab_rects(&self, layout: Layout) -> Vec<(Tab, Rect)> {
        let tabs = [Tab::Results, Tab::Create, Tab::Audit];
        tabs.into_iter()
            .enumerate()
            .map(|(index, tab)| {
                let rect = if layout.wide {
                    Rect {
                        x: layout.navigation.x + 12.0,
                        y: layout.navigation.y + 72.0 + index as f64 * 62.0,
                        width: layout.navigation.width - 24.0,
                        height: 52.0,
                    }
                } else {
                    let weights = [0.28_f64, 0.28_f64, 0.44_f64];
                    let offset = weights[..index].iter().sum::<f64>();
                    let outer = 8.0;
                    let gap = 8.0;
                    let available = layout.navigation.width - outer * 2.0 - gap * 2.0;
                    Rect {
                        x: layout.navigation.x + outer + available * offset + gap * index as f64,
                        y: layout.navigation.y + 8.0,
                        width: available * weights[index],
                        height: 56.0,
                    }
                };
                (tab, rect)
            })
            .collect()
    }

    fn create_rects(&self, layout: Layout) -> (Rect, Rect, Rect, Rect, Rect) {
        let panel_width = if layout.wide {
            layout.content.width.min(860.0)
        } else {
            layout.content.width
        };
        let panel = Rect {
            x: layout.content.x + (layout.content.width - panel_width) / 2.0,
            y: layout.content.y,
            width: panel_width,
            height: layout.content.height,
        };
        let horizontal = if panel.width >= 720.0 { 36.0 } else { 20.0 };
        let top = panel.y + if panel.width >= 720.0 { 104.0 } else { 88.0 } - self.scroll;
        let title = Rect {
            x: panel.x + horizontal,
            y: top,
            width: panel.width - horizontal * 2.0,
            height: 52.0,
        };
        let options = Rect {
            x: title.x,
            y: title.y + 86.0,
            width: title.width,
            height: 128.0,
        };
        let duration = Rect {
            x: title.x,
            y: options.y + options.height + 20.0,
            width: title.width,
            height: 46.0,
        };
        let submit = Rect {
            x: title.x,
            y: duration.y + duration.height + 22.0,
            width: title.width,
            height: 54.0,
        };
        (panel, title, options, duration, submit)
    }

    fn poll_list_rects(&self, layout: Layout) -> Vec<(PublicEvent, Rect)> {
        self.active_polls()
            .into_iter()
            .enumerate()
            .map(|(index, poll)| {
                (
                    poll,
                    Rect {
                        x: layout.content.x + if layout.wide { 28.0 } else { 12.0 },
                        y: layout.content.y + 86.0 + index as f64 * 86.0 - self.scroll,
                        width: layout.content.width - if layout.wide { 56.0 } else { 24.0 },
                        height: 74.0,
                    },
                )
            })
            .collect()
    }

    fn detail_action_rects(&self, layout: Layout) -> (Rect, Rect) {
        let side = if layout.wide { 28.0 } else { 12.0 };
        (
            Rect {
                x: layout.content.x + side,
                y: layout.content.y + 14.0,
                width: 94.0,
                height: 42.0,
            },
            Rect {
                x: layout.content.x + layout.content.width - side - 94.0,
                y: layout.content.y + 14.0,
                width: 94.0,
                height: 42.0,
            },
        )
    }

    fn option_rects(&self, layout: Layout, options: &[OptionDef]) -> Vec<(OptionDef, Rect)> {
        let start_y = layout.content.y + 250.0 - self.scroll;
        options
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, option)| {
                (
                    option,
                    Rect {
                        x: layout.content.x + if layout.wide { 28.0 } else { 12.0 },
                        y: start_y + index as f64 * 72.0,
                        width: layout.content.width - if layout.wide { 56.0 } else { 24.0 },
                        height: 60.0,
                    },
                )
            })
            .collect()
    }

    fn vote_button(&self, layout: Layout) -> Rect {
        Rect {
            x: layout.content.x + if layout.wide { 28.0 } else { 12.0 },
            y: layout.content.y + layout.content.height - 58.0,
            width: layout.content.width - if layout.wide { 56.0 } else { 24.0 },
            height: 48.0,
        }
    }

    fn audit_rows(&self) -> Vec<(PublicEvent, bool, String)> {
        let Some(PublicEvent::PollCreated {
            event_id: poll_id,
            options,
            ..
        }) = self.selected_poll()
        else {
            return Vec::new();
        };
        let winners = self.winning_ballots();
        let winner_ids: BTreeSet<String> = winners
            .values()
            .map(|event| event.id().to_string())
            .collect();
        self.events
            .values()
            .rev()
            .filter_map(|event| match event {
                PublicEvent::Ballot {
                    poll_id: event_poll,
                    option_id,
                    event_id,
                    ..
                } if event_poll == &poll_id => Some((
                    event.clone(),
                    winner_ids.contains(event_id),
                    options
                        .iter()
                        .find(|option| option.id == *option_id)
                        .map(|option| option.label.clone())
                        .unwrap_or_else(|| option_id.clone()),
                )),
                _ => None,
            })
            .collect()
    }

    fn output(&self, include_snapshot: bool) -> Value {
        let state = self.public_state();
        json!({
            "scene":self.scene(),
            "effects":self.effects,
            "events":self.emitted,
            "snapshot":if include_snapshot { serde_json::to_value(&state).unwrap_or(Value::Null) } else { Value::Null },
            "persist":state
        })
    }

    fn scene(&self) -> Value {
        let layout = self.layout();
        let mut draw = vec![
            rect(0.0, 0.0, layout.width, layout.height, 0.0, "#08111f", ""),
            rect(
                layout.header.x,
                layout.header.y,
                layout.header.width,
                layout.header.height,
                0.0,
                "#0d1728",
                "#1e293b",
            ),
            text("共享投票", 20.0, 29.0, 24.0, 700, "#f8fafc", "left", 210.0),
            text(
                if layout.wide {
                    "P2P 本地统计 · 可公开审计"
                } else {
                    "本地统计 · 公开审计"
                },
                20.0,
                56.0,
                13.0,
                500,
                "#a8b6c8",
                "left",
                (layout.width - 128.0).max(100.0),
            ),
            rect(
                layout.fullscreen.x,
                layout.fullscreen.y,
                layout.fullscreen.width,
                layout.fullscreen.height,
                12.0,
                "#17233a",
                "#334155",
            ),
            text(
                if self.fullscreen { "退出" } else { "全屏" },
                layout.fullscreen.x + layout.fullscreen.width / 2.0,
                layout.fullscreen.y + 24.0,
                14.0,
                700,
                "#dbeafe",
                "center",
                layout.fullscreen.width - 8.0,
            ),
        ];

        draw.push(rect(
            layout.navigation.x,
            layout.navigation.y,
            layout.navigation.width,
            layout.navigation.height,
            16.0,
            "#0d1728",
            "#1e293b",
        ));
        if layout.wide {
            draw.push(text(
                "导航",
                layout.navigation.x + 18.0,
                layout.navigation.y + 26.0,
                12.0,
                700,
                "#64748b",
                "left",
                100.0,
            ));
        }
        for (tab, item) in self.tab_rects(layout) {
            let selected = self.tab == tab;
            draw.push(rect(
                item.x,
                item.y,
                item.width,
                item.height,
                12.0,
                if selected { "#1d4ed8" } else { "#111c30" },
                if selected { "#60a5fa" } else { "#1e293b" },
            ));
            draw.push(text(
                tab.label(),
                item.x + item.width / 2.0,
                item.y + item.height / 2.0,
                15.0,
                if selected { 700 } else { 600 },
                if selected { "#ffffff" } else { "#cbd5e1" },
                "center",
                item.width - 12.0,
            ));
        }

        draw.push(rect(
            layout.content.x,
            layout.content.y,
            layout.content.width,
            layout.content.height,
            16.0,
            "#0d1728",
            "#1e293b",
        ));
        draw.push(json!({"op":"clip-push","x":layout.content.x,"y":layout.content.y,"width":layout.content.width,"height":layout.content.height,"radius":16}));
        match self.tab {
            Tab::Create => self.draw_create(layout, &mut draw),
            Tab::Results => self.draw_results(layout, &mut draw),
            Tab::Audit => self.draw_audit(layout, &mut draw),
        }
        draw.push(json!({"op":"clip-pop"}));
        json!({
            "width":layout.width,"height":layout.height,"background":"#08111f",
            "cursor":"default","draw":draw
        })
    }

    fn draw_create(&self, layout: Layout, draw: &mut Vec<Value>) {
        let (panel, title_rect, option_rect, duration_rect, submit) = self.create_rects(layout);
        draw.push(text(
            "创建公开投票",
            panel.x + if panel.width >= 720.0 { 36.0 } else { 20.0 },
            panel.y + 38.0 - self.scroll,
            if panel.width >= 720.0 { 28.0 } else { 22.0 },
            700,
            "#f8fafc",
            "left",
            panel.width - 32.0,
        ));
        if panel.width >= 620.0 {
            draw.push(text(
                "标题和选项会成为可转发、可排重的公开事件。",
                panel.x + if panel.width >= 720.0 { 36.0 } else { 20.0 },
                panel.y + 72.0 - self.scroll,
                14.0,
                500,
                "#a8b6c8",
                "left",
                panel.width - 56.0,
            ));
        }
        draw.push(text(
            "投票标题",
            title_rect.x,
            title_rect.y - 20.0,
            14.0,
            600,
            "#cbd5e1",
            "left",
            title_rect.width,
        ));
        draw.push(rect(
            title_rect.x,
            title_rect.y,
            title_rect.width,
            title_rect.height,
            12.0,
            "#111c30",
            if self.focused == "poll-title" {
                "#60a5fa"
            } else {
                "#334155"
            },
        ));
        draw.push(text(
            if self.draft_title.is_empty() {
                "点击输入标题"
            } else {
                &self.draft_title
            },
            title_rect.x + 14.0,
            title_rect.y + title_rect.height / 2.0,
            16.0,
            500,
            if self.draft_title.is_empty() {
                "#64748b"
            } else {
                "#f8fafc"
            },
            "left",
            title_rect.width - 28.0,
        ));

        draw.push(text(
            "选项（每行一项，2–32 项）",
            option_rect.x,
            option_rect.y - 20.0,
            14.0,
            600,
            "#cbd5e1",
            "left",
            option_rect.width,
        ));
        draw.push(rect(
            option_rect.x,
            option_rect.y,
            option_rect.width,
            option_rect.height,
            12.0,
            "#111c30",
            if self.focused == "poll-options" {
                "#60a5fa"
            } else {
                "#334155"
            },
        ));
        let lines: Vec<String> = if self.draft_options.trim().is_empty() {
            vec!["点击输入选项".into(), "例如：选项 A / 选项 B".into()]
        } else {
            self.draft_options
                .lines()
                .take(4)
                .map(|line| line.chars().take(48).collect())
                .collect()
        };
        for (index, line) in lines.iter().enumerate() {
            draw.push(text(
                line,
                option_rect.x + 14.0,
                option_rect.y + 25.0 + index as f64 * 27.0,
                16.0,
                500,
                if self.draft_options.is_empty() {
                    "#64748b"
                } else {
                    "#e2e8f0"
                },
                "left",
                option_rect.width - 28.0,
            ));
        }
        draw.push(text(
            "有效期",
            duration_rect.x,
            duration_rect.y - 13.0,
            13.0,
            600,
            "#cbd5e1",
            "left",
            duration_rect.width,
        ));
        draw.push(rect(
            duration_rect.x,
            duration_rect.y,
            duration_rect.width,
            duration_rect.height,
            12.0,
            "#17233a",
            "#3b82f6",
        ));
        draw.push(text(
            &format!(
                "{} 天 · 点击切换（最长 14 天，到期自动删除）",
                self.draft_duration_days
            ),
            duration_rect.x + 14.0,
            duration_rect.y + duration_rect.height / 2.0,
            13.0,
            650,
            "#bfdbfe",
            "left",
            duration_rect.width - 28.0,
        ));
        draw.push(rect(
            submit.x,
            submit.y,
            submit.width,
            submit.height,
            12.0,
            "#2563eb",
            "#60a5fa",
        ));
        draw.push(text(
            "创建并广播",
            submit.x + submit.width / 2.0,
            submit.y + submit.height / 2.0,
            16.0,
            700,
            "#ffffff",
            "center",
            submit.width - 16.0,
        ));
    }

    fn draw_poll_list(&self, layout: Layout, draw: &mut Vec<Value>) {
        let polls = self.active_polls();
        let left = layout.content.x + if layout.wide { 28.0 } else { 14.0 };
        draw.push(text(
            "可参与的投票",
            left,
            layout.content.y + 28.0,
            if layout.wide { 24.0 } else { 19.0 },
            750,
            "#f8fafc",
            "left",
            layout.content.width - 40.0,
        ));
        draw.push(text(
            &format!("{} 个有效投票 · 最长保留 14 天", polls.len()),
            left,
            layout.content.y + 58.0,
            12.0,
            550,
            "#94a3b8",
            "left",
            layout.content.width - 40.0,
        ));
        if polls.is_empty() {
            draw.push(text(
                "暂无有效投票",
                layout.content.x + layout.content.width / 2.0,
                layout.content.y + layout.content.height / 2.0 - 18.0,
                20.0,
                750,
                "#f8fafc",
                "center",
                layout.content.width - 40.0,
            ));
            draw.push(text(
                "切换到“创建”发起一个投票",
                layout.content.x + layout.content.width / 2.0,
                layout.content.y + layout.content.height / 2.0 + 18.0,
                14.0,
                500,
                "#94a3b8",
                "center",
                layout.content.width - 40.0,
            ));
            return;
        }
        for (poll, item) in self.poll_list_rects(layout) {
            if item.y + item.height < layout.content.y + 72.0
                || item.y > layout.content.y + layout.content.height
            {
                continue;
            }
            if let PublicEvent::PollCreated {
                event_id,
                creator_hash,
                nick,
                title,
                options,
                expires_at_ms,
                ..
            } = poll
            {
                let voters = self.winning_ballots_for(&event_id, &options).len();
                draw.push(rect(
                    item.x,
                    item.y,
                    item.width,
                    item.height,
                    12.0,
                    "#111c30",
                    if creator_hash == self.voter_hash {
                        "#3b82f6"
                    } else {
                        "#334155"
                    },
                ));
                draw.push(text(
                    &title,
                    item.x + 14.0,
                    item.y + 22.0,
                    15.0,
                    700,
                    "#f8fafc",
                    "left",
                    item.width - 120.0,
                ));
                draw.push(text(
                    &format!("{} 人", voters),
                    item.x + item.width - 14.0,
                    item.y + 22.0,
                    12.0,
                    750,
                    "#93c5fd",
                    "right",
                    90.0,
                ));
                draw.push(text(
                    &format!(
                        "{} · Hash {} · {}",
                        nick,
                        short(&creator_hash, 10),
                        remaining_label(self.now_ms, expires_at_ms)
                    ),
                    item.x + 14.0,
                    item.y + 51.0,
                    12.0,
                    500,
                    "#94a3b8",
                    "left",
                    item.width - 28.0,
                ));
            }
        }
    }

    fn draw_results(&self, layout: Layout, draw: &mut Vec<Value>) {
        if self.selected_poll().is_none() {
            self.draw_poll_list(layout, draw);
            return;
        }
        let Some(PublicEvent::PollCreated {
            title,
            options,
            event_id,
            expires_at_ms,
            ..
        }) = self.selected_poll()
        else {
            return;
        };
        let (back, delete) = self.detail_action_rects(layout);
        for (button, label, danger) in [(back, "返回列表", false), (delete, "删除投票", true)]
        {
            if danger && !self.can_delete_selected() {
                continue;
            }
            draw.push(rect(
                button.x,
                button.y,
                button.width,
                button.height,
                10.0,
                if danger { "#3b1822" } else { "#17233a" },
                if danger { "#7f1d1d" } else { "#334155" },
            ));
            draw.push(text(
                label,
                button.x + button.width / 2.0,
                button.y + button.height / 2.0,
                13.0,
                700,
                if danger { "#fecaca" } else { "#dbeafe" },
                "center",
                button.width - 12.0,
            ));
        }
        let counts = self.option_counts(&options);
        let total = self.winning_ballots().len();
        draw.push(text(
            &title,
            layout.content.x + if layout.wide { 28.0 } else { 16.0 },
            layout.content.y + 82.0,
            if layout.wide { 24.0 } else { 19.0 },
            750,
            "#f8fafc",
            "left",
            layout.content.width - 42.0,
        ));
        draw.push(text(
            &format!(
                "{total} 个有效 voter hash · {} · Poll {}",
                remaining_label(self.now_ms, expires_at_ms),
                short(&event_id, 10)
            ),
            layout.content.x + if layout.wide { 28.0 } else { 16.0 },
            layout.content.y + 110.0,
            12.0,
            550,
            "#94a3b8",
            "left",
            layout.content.width - 42.0,
        ));
        let notice = Rect {
            x: layout.content.x + if layout.wide { 28.0 } else { 12.0 },
            y: layout.content.y + 132.0,
            width: layout.content.width - if layout.wide { 56.0 } else { 24.0 },
            height: 96.0,
        };
        draw.push(rect(
            notice.x,
            notice.y,
            notice.width,
            notice.height,
            14.0,
            "#17233a",
            "#1d4ed8",
        ));
        draw.push(text(
            "本地收集视图",
            notice.x + 14.0,
            notice.y + 20.0,
            14.0,
            750,
            "#dbeafe",
            "left",
            notice.width - 28.0,
        ));
        draw.push(text(
            "不同人看到的统计可能不同，取决于各自收到的事件。",
            notice.x + 14.0,
            notice.y + 47.0,
            12.0,
            550,
            "#bfdbfe",
            "left",
            notice.width - 28.0,
        ));
        draw.push(text(
            &format!(
                "你的 Hash {} · 同一 Hash 只计最新 revision",
                short(&self.voter_hash, 14)
            ),
            notice.x + 14.0,
            notice.y + 73.0,
            12.0,
            550,
            "#93c5fd",
            "left",
            notice.width - 28.0,
        ));

        for (option, item) in self.option_rects(layout, &options) {
            if item.y + item.height < layout.content.y + 228.0
                || item.y > layout.content.y + layout.content.height - 68.0
            {
                continue;
            }
            let selected = self.selected_option.as_deref() == Some(option.id.as_str());
            let count = *counts.get(&option.id).unwrap_or(&0);
            draw.push(rect(
                item.x,
                item.y,
                item.width,
                item.height,
                12.0,
                if selected { "#172554" } else { "#111c30" },
                if selected { "#60a5fa" } else { "#334155" },
            ));
            draw.push(text(
                &option.label,
                item.x + 14.0,
                item.y + 20.0,
                14.0,
                650,
                "#f8fafc",
                "left",
                item.width - 92.0,
            ));
            draw.push(text(
                &format!("{count} 票"),
                item.x + item.width - 14.0,
                item.y + 20.0,
                13.0,
                750,
                "#dbeafe",
                "right",
                70.0,
            ));
            let bar_width = if total == 0 {
                0.0
            } else {
                (item.width - 28.0) * count as f64 / total as f64
            };
            draw.push(rect(
                item.x + 14.0,
                item.y + 42.0,
                item.width - 28.0,
                7.0,
                4.0,
                "#1e293b",
                "",
            ));
            if bar_width > 0.0 {
                draw.push(rect(
                    item.x + 14.0,
                    item.y + 42.0,
                    bar_width,
                    7.0,
                    4.0,
                    "#3b82f6",
                    "",
                ));
            }
        }
        let vote = self.vote_button(layout);
        draw.push(rect(
            vote.x,
            vote.y,
            vote.width,
            vote.height,
            12.0,
            if self.selected_option.is_some() {
                "#f97316"
            } else {
                "#263244"
            },
            if self.selected_option.is_some() {
                "#fdba74"
            } else {
                "#475569"
            },
        ));
        draw.push(text(
            if self.selected_option.is_some() {
                "提交公开选票"
            } else {
                "请先选择一个选项"
            },
            vote.x + vote.width / 2.0,
            vote.y + vote.height / 2.0,
            15.0,
            750,
            if self.selected_option.is_some() {
                "#ffffff"
            } else {
                "#94a3b8"
            },
            "center",
            vote.width - 18.0,
        ));
    }

    fn draw_audit(&self, layout: Layout, draw: &mut Vec<Value>) {
        let rows = self.audit_rows();
        let left = layout.content.x + if layout.wide { 28.0 } else { 14.0 };
        draw.push(text(
            "公开原始票据",
            left,
            layout.content.y + 28.0,
            if layout.wide { 24.0 } else { 19.0 },
            750,
            "#f8fafc",
            "left",
            layout.content.width - 40.0,
        ));
        draw.push(text(
            &format!(
                "{} 条 ballot 事件 · {} 个当前有效 Hash",
                rows.len(),
                self.winning_ballots().len()
            ),
            left,
            layout.content.y + 58.0,
            12.0,
            550,
            "#94a3b8",
            "left",
            layout.content.width - 40.0,
        ));
        if rows.is_empty() {
            draw.push(text(
                "还没有收到选票",
                layout.content.x + layout.content.width / 2.0,
                layout.content.y + layout.content.height / 2.0,
                16.0,
                650,
                "#94a3b8",
                "center",
                layout.content.width - 40.0,
            ));
            return;
        }
        for (index, (event, current, label)) in rows.into_iter().enumerate() {
            let y = layout.content.y + 84.0 + index as f64 * 72.0 - self.scroll;
            if y + 62.0 < layout.content.y + 72.0 || y > layout.content.y + layout.content.height {
                continue;
            }
            let row = Rect {
                x: left,
                y,
                width: layout.content.width - (left - layout.content.x) * 2.0,
                height: 62.0,
            };
            draw.push(rect(
                row.x,
                row.y,
                row.width,
                row.height,
                12.0,
                "#111c30",
                if current { "#2563eb" } else { "#334155" },
            ));
            if let PublicEvent::Ballot {
                nick,
                voter_hash,
                revision,
                event_id,
                ..
            } = event
            {
                draw.push(text(
                    &format!("{} · Hash {}", nick, short(&voter_hash, 12)),
                    row.x + 12.0,
                    row.y + 18.0,
                    13.0,
                    650,
                    "#e2e8f0",
                    "left",
                    row.width - 110.0,
                ));
                draw.push(text(
                    if current {
                        "当前计票"
                    } else {
                        "已被改票替代"
                    },
                    row.x + row.width - 12.0,
                    row.y + 18.0,
                    11.0,
                    700,
                    if current { "#93c5fd" } else { "#94a3b8" },
                    "right",
                    100.0,
                ));
                draw.push(text(
                    &format!(
                        "{} · revision {} · Event {}",
                        label,
                        revision,
                        short(&event_id, 10)
                    ),
                    row.x + 12.0,
                    row.y + 43.0,
                    12.0,
                    500,
                    "#94a3b8",
                    "left",
                    row.width - 24.0,
                ));
            }
        }
    }
}

fn canonical_options(options: &[OptionDef]) -> String {
    options
        .iter()
        .map(|option| format!("{}\0{}", option.id, option.label))
        .collect::<Vec<_>>()
        .join("\0")
}

fn clean(value: &str, max: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= max).then(|| value.to_string())
}

fn valid_seed(seed: &str) -> bool {
    seed.len() == 64 && seed.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_option_id(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_event(event: &PublicEvent) -> bool {
    match event {
        PublicEvent::PollCreated {
            event_id,
            creator_hash,
            nick,
            title,
            options,
            created_at_ms,
            expires_at_ms,
        } => {
            if !valid_hash(event_id)
                || !valid_hash(creator_hash)
                || clean(nick, 80).as_deref() != Some(nick)
                || clean(title, 200).as_deref() != Some(title)
                || options.len() < 2
                || options.len() > 32
                || *created_at_ms == 0
                || *expires_at_ms <= *created_at_ms
                || expires_at_ms.saturating_sub(*created_at_ms) > MAX_POLL_DURATION_MS
            {
                return false;
            }
            let mut ids = BTreeSet::new();
            let mut labels = BTreeSet::new();
            if options.iter().enumerate().any(|(index, option)| {
                !valid_option_id(&option.id)
                    || clean(&option.label, 120).as_deref() != Some(option.label.as_str())
                    || option.id
                        != sha256_hex(format!("option\0{index}\0{}", option.label).as_bytes())[..16]
                    || !ids.insert(option.id.clone())
                    || !labels.insert(option.label.clone())
            }) {
                return false;
            }
            *event_id
                == sha256_hex(
                    format!(
                        "poll-created\0{}\0{}\0{}\0{}\0{}\0{}",
                        creator_hash,
                        nick,
                        title,
                        canonical_options(options),
                        created_at_ms,
                        expires_at_ms
                    )
                    .as_bytes(),
                )
        }
        PublicEvent::Ballot {
            event_id,
            poll_id,
            expires_at_ms,
            voter_hash,
            nick,
            option_id,
            revision,
        } => {
            *revision > 0
                && valid_hash(event_id)
                && valid_hash(poll_id)
                && valid_hash(voter_hash)
                && valid_option_id(option_id)
                && clean(nick, 80).as_deref() == Some(nick)
                && *event_id
                    == sha256_hex(
                        format!(
                            "ballot\0{}\0{}\0{}\0{}\0{}\0{}",
                            poll_id, expires_at_ms, voter_hash, nick, option_id, revision
                        )
                        .as_bytes(),
                    )
        }
        PublicEvent::PollDeleted {
            event_id,
            poll_id,
            expires_at_ms,
            creator_hash,
        } => {
            valid_hash(event_id)
                && valid_hash(poll_id)
                && valid_hash(creator_hash)
                && *event_id
                    == sha256_hex(
                        format!(
                            "poll-deleted\0{}\0{}\0{}",
                            poll_id, expires_at_ms, creator_hash
                        )
                        .as_bytes(),
                    )
        }
    }
}

fn remaining_label(now_ms: u64, expires_at_ms: u64) -> String {
    let remaining = expires_at_ms.saturating_sub(now_ms);
    if remaining == 0 {
        return "已到期".into();
    }
    let hours = remaining.saturating_add(60 * 60 * 1_000 - 1) / (60 * 60 * 1_000);
    if hours >= 24 {
        format!("剩余 {} 天", hours.saturating_add(23) / 24)
    } else {
        format!("剩余 {hours} 小时")
    }
}

fn short(value: &str, chars: usize) -> String {
    value.chars().take(chars).collect()
}

fn rect(x: f64, y: f64, width: f64, height: f64, radius: f64, fill: &str, stroke: &str) -> Value {
    json!({"op":"rect","x":x,"y":y,"width":width,"height":height,"radius":radius,"fill":fill,"stroke":stroke,"lineWidth":1})
}

#[allow(clippy::too_many_arguments)]
fn text(
    value: &str,
    x: f64,
    y: f64,
    size: f64,
    weight: u16,
    color: &str,
    align: &str,
    max_width: f64,
) -> Value {
    json!({"op":"text","text":value,"x":x,"y":y,"size":size,"weight":weight,"color":color,"align":align,"baseline":"middle","maxWidth":max_width})
}

pub fn sha256_hex(input: &[u8]) -> String {
    let digest = sha256(input);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut data = input.to_vec();
    let bit_len = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in chunk.chunks_exact(4).enumerate() {
            w[index] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut output = [0u8; 32];
    for (index, word) in h.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

thread_local! {
    static APP: RefCell<Option<VotingApp>> = const { RefCell::new(None) };
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn set_output(value: Value) {
    OUTPUT.with(|output| {
        *output.borrow_mut() = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec())
    });
}

fn parse_input(pointer: u32, length: u32) -> Result<Value, String> {
    if pointer == 0 || length == 0 || length > 2 * 1024 * 1024 {
        return Err("invalid input".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer as *const u8, length as usize) };
    serde_json::from_slice(bytes).map_err(|_| "invalid JSON".into())
}

#[no_mangle]
pub extern "C" fn rh_abi_version() -> u32 {
    3
}

#[no_mangle]
pub extern "C" fn rh_alloc(length: u32) -> u32 {
    if length == 0 || length > 2 * 1024 * 1024 {
        return 0;
    }
    let bytes = vec![0_u8; length as usize].into_boxed_slice();
    Box::into_raw(bytes) as *mut u8 as u32
}

#[no_mangle]
pub extern "C" fn rh_dealloc(pointer: u32, length: u32) {
    if pointer != 0 && length > 0 && length <= 2 * 1024 * 1024 {
        let slice = std::ptr::slice_from_raw_parts_mut(pointer as *mut u8, length as usize);
        unsafe {
            drop(Box::from_raw(slice));
        }
    }
}

#[no_mangle]
pub extern "C" fn rh_init(pointer: u32, length: u32) -> u32 {
    let result = parse_input(pointer, length)
        .and_then(|value| {
            serde_json::from_value::<InitContext>(value).map_err(|_| "invalid init context".into())
        })
        .and_then(VotingApp::new);
    match result {
        Ok(app) => {
            let output = app.output(false);
            APP.with(|slot| *slot.borrow_mut() = Some(app));
            set_output(output);
            1
        }
        Err(error) => {
            set_output(json!({"error":error}));
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn rh_dispatch(pointer: u32, length: u32) -> u32 {
    let result = parse_input(pointer, length).and_then(|input| {
        APP.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .ok_or_else(|| "not initialized".to_string())?
                .dispatch(input)
        })
    });
    match result {
        Ok(output) => {
            set_output(output);
            1
        }
        Err(error) => {
            set_output(json!({"error":error}));
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn rh_output_ptr() -> u32 {
    OUTPUT.with(|output| output.borrow().as_ptr() as u32)
}

#[no_mangle]
pub extern "C" fn rh_output_len() -> u32 {
    OUTPUT.with(|output| output.borrow().len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(seed: char, nick: &str) -> InitContext {
        InitContext {
            nickname: nick.into(),
            peer_id: String::new(),
            identity_seed: seed.to_string().repeat(64),
            channel_id: String::new(),
            instance_id: String::new(),
            locale: "zh-CN".into(),
            theme: "dark".into(),
            saved_state: None,
            now_ms: 1_800_000_000_000,
        }
    }

    fn create(app: &mut VotingApp) -> PublicEvent {
        app.draft_title = "午饭".into();
        app.draft_options = "面\n饭\n沙拉".into();
        app.submit_create();
        app.emitted.remove(0)
    }

    fn options(app: &VotingApp) -> Vec<OptionDef> {
        let Some(PublicEvent::PollCreated { options, .. }) = app.selected_poll() else {
            unreachable!()
        };
        options
    }

    fn cast(app: &mut VotingApp, option_id: &str) -> PublicEvent {
        app.selected_option = Some(option_id.into());
        app.submit_vote();
        app.emitted.remove(0)
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn duplicate_hash_changes_vote_without_double_counting() {
        let mut app = VotingApp::new(context('a', "Alice")).unwrap();
        create(&mut app);
        let choices = options(&app);
        cast(&mut app, &choices[0].id);
        cast(&mut app, &choices[1].id);
        let winners = app.winning_ballots();
        assert_eq!(winners.len(), 1);
        let PublicEvent::Ballot {
            option_id,
            revision,
            ..
        } = winners.values().next().unwrap()
        else {
            unreachable!()
        };
        assert_eq!(option_id, &choices[1].id);
        assert_eq!(*revision, 2);
        assert_eq!(app.audit_rows().len(), 2);
    }

    #[test]
    fn two_instances_converge_under_reordered_events() {
        let mut a = VotingApp::new(context('a', "Alice")).unwrap();
        let poll = create(&mut a);
        let poll_id = poll.id().to_string();
        let choices = options(&a);
        let alice_old = cast(&mut a, &choices[0].id);
        let alice_new = cast(&mut a, &choices[2].id);
        let mut b = VotingApp::new(context('b', "Bob")).unwrap();
        b.dispatch(json!({"kind":"remote","event":poll.clone()}))
            .unwrap();
        b.selected_poll_id = Some(poll_id);
        let bob = cast(&mut b, &choices[1].id);
        a.dispatch(json!({"kind":"remote","event":bob.clone()}))
            .unwrap();
        b.dispatch(json!({"kind":"remote","event":alice_new.clone()}))
            .unwrap();
        b.dispatch(json!({"kind":"remote","event":alice_old.clone()}))
            .unwrap();
        a.dispatch(json!({"kind":"remote","event":alice_old}))
            .unwrap();
        a.dispatch(json!({"kind":"remote","event":alice_new}))
            .unwrap();
        b.dispatch(json!({"kind":"remote","event":bob})).unwrap();
        assert_eq!(a.public_state(), b.public_state());
        assert_eq!(a.winning_ballots(), b.winning_ballots());
    }

    #[test]
    fn snapshot_merges_missing_events() {
        let mut a = VotingApp::new(context('a', "Alice")).unwrap();
        create(&mut a);
        let choice = options(&a)[0].id.clone();
        cast(&mut a, &choice);
        let mut b = VotingApp::new(context('b', "Bob")).unwrap();
        b.dispatch(json!({"kind":"snapshot","state":a.public_state()}))
            .unwrap();
        assert_eq!(a.public_state(), b.public_state());
    }

    #[test]
    fn lists_multiple_polls_and_creator_can_delete_their_own() {
        let mut app = VotingApp::new(context('a', "Alice")).unwrap();
        let first = create(&mut app);
        let first_id = first.id().to_string();
        app.now_ms += 1_000;
        app.draft_title = "周末活动".into();
        app.draft_options = "爬山\n看电影".into();
        app.submit_create();
        assert_eq!(app.active_polls().len(), 2);
        app.selected_poll_id = None;
        let scene = app.scene().to_string();
        assert!(scene.contains("午饭") && scene.contains("周末活动"));

        app.selected_poll_id = Some(first_id.clone());
        assert!(app.can_delete_selected());
        app.delete_selected_poll();
        assert_eq!(app.active_polls().len(), 1);
        assert!(!app.public_state().events.iter().any(|event| {
            matches!(event, PublicEvent::PollCreated { event_id, .. } if event_id == &first_id)
                || matches!(event, PublicEvent::Ballot { poll_id, .. } if poll_id == &first_id)
        }));
        assert!(app
            .emitted
            .iter()
            .any(|event| matches!(event, PublicEvent::PollDeleted { poll_id, .. } if poll_id == &first_id)));
    }

    #[test]
    fn expiry_removes_poll_content_and_ballots_after_fourteen_days() {
        let mut app = VotingApp::new(context('a', "Alice")).unwrap();
        app.draft_duration_days = 14;
        create(&mut app);
        let choice = options(&app)[0].id.clone();
        cast(&mut app, &choice);
        assert_eq!(app.public_state().events.len(), 2);

        app.dispatch(json!({
            "kind":"viewport","width":960,"height":640,"fullscreen":false,
            "nowMs":app.now_ms + MAX_POLL_DURATION_MS + 1
        }))
        .unwrap();
        assert!(app.active_polls().is_empty());
        assert!(app.public_state().events.is_empty());
        assert!(app.selected_poll_id.is_none());
    }

    #[test]
    fn responsive_scene_and_app_owned_fullscreen() {
        let mut app = VotingApp::new(context('a', "Alice")).unwrap();
        for (width, height) in [
            (320.0, 480.0),
            (375.0, 812.0),
            (768.0, 1024.0),
            (1440.0, 900.0),
        ] {
            let output = app.dispatch(json!({"kind":"viewport","width":width,"height":height,"dpr":2,"fullscreen":false})).unwrap();
            assert_eq!(output["scene"]["width"], width);
            assert_eq!(output["scene"]["height"], height);
            assert!(!output.to_string().contains("roomhash-form"));
        }
        let layout = app.layout();
        let output = app.dispatch(json!({"kind":"pointer","phase":"down","pointerId":1,"x":layout.fullscreen.x+10.0,"y":layout.fullscreen.y+10.0,"buttons":1,"pressure":0.5})).unwrap();
        assert_eq!(output["effects"][0]["type"], "fullscreen");
    }

    #[test]
    fn text_fields_are_requested_by_wasm_ui() {
        let mut app = VotingApp::new(context('a', "Alice")).unwrap();
        let layout = app.layout();
        let (_, title, _, _, _) = app.create_rects(layout);
        let output = app.dispatch(json!({"kind":"pointer","phase":"down","pointerId":1,"x":title.x+8.0,"y":title.y+8.0,"buttons":1,"pressure":0.5})).unwrap();
        assert_eq!(output["effects"][0]["type"], "text-input");
        assert_eq!(output["effects"][0]["requestId"], "poll-title");
    }
}
