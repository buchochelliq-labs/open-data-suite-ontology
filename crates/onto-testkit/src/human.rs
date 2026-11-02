//! The person in a conversation (feature `llm`).
//!
//! Either a [`ScriptedHuman`] with fixed replies, or a [`SimulatedHuman`]: a
//! second model playing the person from a task's [`Persona`]. The simulated
//! person sees only what a person would see (the messages, not tool calls),
//! knows only what the persona says, and decides when it is satisfied and
//! whether to accept a proposal. One model plays the agent, another the person.
//!
//! The person does not reread the whole conversation on every reply: by
//! default it sees its opening message and the last [`DEFAULT_WINDOW`]
//! messages, with those in between summarised as "(n earlier messages)"
//! ([`PersonHistory`]). Its persona, in the system prompt, says what it wants
//! and knows, so the middle of a long exchange is rarely needed.

use anyhow::Result;
use serde::Deserialize;

use crate::agent::{Reply, ScriptedHuman, TokenUsage};
use crate::eval::{Agent, ask};
use crate::transcript::{Event, Transcript};

/// Someone the agent talks to. Asked for a reply whenever the agent stops.
pub trait Person {
    fn reply(&mut self, transcript: &Transcript) -> impl Future<Output = Result<Reply>>;
}

impl Person for ScriptedHuman {
    async fn reply(&mut self, _transcript: &Transcript) -> Result<Reply> {
        Ok(self.next_reply())
    }
}

/// Who the simulated person is, written per task.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Persona {
    /// Their job, e.g. "analytics engineer on the finance team".
    pub role: String,
    /// What they want from the conversation.
    pub goal: String,
    /// What they know and can tell the agent when it is relevant or asked.
    #[serde(default)]
    pub knows: Vec<String>,
    /// Whether they accept a proposal that records something they said.
    #[serde(default)]
    pub accepts_proposals: bool,
    /// Replies before they end the conversation regardless (default 3).
    #[serde(default)]
    pub max_replies: Option<usize>,
}

impl Persona {
    fn system_prompt(&self) -> String {
        let knows = if self.knows.is_empty() {
            "Nothing beyond your goal.".to_string()
        } else {
            self.knows.iter().map(|k| format!("- {k}")).collect::<Vec<_>>().join("\n")
        };
        let accept = if self.accepts_proposals {
            "If the assistant has prepared a proposal that records something you said, and it is correct, \
             start your reply with ACCEPT on its own line, then say anything else you want to."
        } else {
            "You do not accept or approve proposals in this conversation."
        };
        format!(
            "You are role-playing a person who is talking to an AI data-modelling assistant. Stay in character; \
             never mention that you are role-playing or an AI.\n\n\
             Who you are: {role}\nWhat you want: {goal}\n\nWhat you know (share it when it is relevant or you are asked; \
             do not volunteer everything at once):\n{knows}\n\n\
             You know nothing else about the data: never invent table names, grains, keys or decisions that are \
             not listed above. If you are asked something you do not know, say so plainly. You cannot run tools, \
             queries or commands, and you do not leave the conversation to check things; only the assistant can look \
             things up. Speak to the assistant as \"you\". Reply like a busy colleague: one to three sentences.\n\n\
             {accept}\n\nWhen you have what you need, or the assistant has nothing more to offer, reply with only: DONE",
            role = self.role,
            goal = self.goal,
        )
    }
}

/// Messages a simulated person sees after its opening message, by default.
pub const DEFAULT_WINDOW: usize = 4;

/// How much of the conversation the simulated person sees on each reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonHistory {
    /// Every message, as before windowing (for comparison; costs more).
    Full,
    /// The opening message and the last `n` messages (at least 1).
    Window(usize),
}

impl Default for PersonHistory {
    fn default() -> Self {
        PersonHistory::Window(DEFAULT_WINDOW)
    }
}

impl std::fmt::Display for PersonHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersonHistory::Full => f.write_str("full"),
            PersonHistory::Window(n) => write!(f, "{n}"),
        }
    }
}

impl std::str::FromStr for PersonHistory {
    type Err = String;

    /// `full`, or the number of latest messages to show (at least 1).
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("full") {
            return Ok(PersonHistory::Full);
        }
        match s.parse::<usize>() {
            Ok(n) if n > 0 => Ok(PersonHistory::Window(n)),
            _ => Err(format!("invalid person history `{s}`: expected `full` or a number of messages of at least 1")),
        }
    }
}

/// A model playing the person.
pub struct SimulatedHuman {
    persona: Persona,
    model: Agent,
    history: PersonHistory,
    replies: usize,
    /// Who accepts proposals, recorded on the acceptance.
    pub accepted_by: String,
    /// Tokens the person's model used so far.
    pub usage: TokenUsage,
}

impl SimulatedHuman {
    pub fn new(persona: Persona, model: Agent) -> Self {
        Self { persona, model, history: PersonHistory::default(), replies: 0, accepted_by: "simulated-person".into(), usage: TokenUsage::default() }
    }

    /// How much of the conversation the person sees on each reply.
    pub fn with_history(mut self, history: PersonHistory) -> Self {
        self.history = history;
        self
    }
}

/// The messages a person saw, in order: theirs, the assistant's and their own
/// actions, no tool calls.
fn person_messages(transcript: &Transcript) -> Vec<String> {
    transcript
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Human { text } => Some(format!("YOU: {text}")),
            Event::Agent { text } => Some(format!("ASSISTANT: {text}")),
            Event::HumanAction { action, detail } => Some(format!("(you did: {action} {detail})")),
            Event::ToolCall { .. } => None,
        })
        .collect()
}

/// The whole conversation as the person saw it.
pub fn person_view(transcript: &Transcript) -> String {
    person_view_with(transcript, PersonHistory::Full)
}

/// The conversation as the person sees it under `history`. With a window of
/// `n`, the opening message and the last `n` messages are shown, and the
/// messages between them are replaced by one line, "(k earlier messages)".
/// Nothing is left out while the conversation fits in the window.
pub fn person_view_with(transcript: &Transcript, history: PersonHistory) -> String {
    let mut messages = person_messages(transcript);
    if let PersonHistory::Window(n) = history {
        let omitted = messages.len().saturating_sub(n + 1);
        if omitted > 0 {
            let plural = if omitted == 1 { "" } else { "s" };
            messages.splice(1..1 + omitted, [format!("({omitted} earlier message{plural})")]);
        }
    }
    messages.join("\n\n")
}

/// The request the person's model gets for its next reply.
pub fn person_prompt(transcript: &Transcript, history: PersonHistory) -> String {
    format!("The conversation so far:\n\n{}\n\nWrite your next message to the assistant, or DONE.", person_view_with(transcript, history))
}

/// Read the person model's answer: `DONE` ends, a leading `ACCEPT` line accepts.
pub fn parse_reply(text: &str, may_accept: bool, by: &str) -> Reply {
    let text = text.trim().trim_matches('"').trim();
    if text.is_empty() || text.eq_ignore_ascii_case("done") || text.trim_end_matches('.').eq_ignore_ascii_case("done") {
        return Reply::End;
    }
    let mut lines = text.lines();
    if may_accept && lines.next().is_some_and(|first| first.trim().trim_end_matches('.').eq_ignore_ascii_case("accept")) {
        let rest = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        return Reply::AcceptLastProposal { by: by.to_string(), then: (!rest.is_empty()).then_some(rest) };
    }
    Reply::Say(text.to_string())
}

impl Person for SimulatedHuman {
    async fn reply(&mut self, transcript: &Transcript) -> Result<Reply> {
        if self.replies >= self.persona.max_replies.unwrap_or(3) {
            return Ok(Reply::End);
        }
        self.replies += 1;
        let prompt = person_prompt(transcript, self.history);
        let (text, usage) = ask(&self.model, &self.persona.system_prompt(), &prompt).await?;
        self.usage += usage;
        Ok(parse_reply(&text, self.persona.accepts_proposals, &self.accepted_by))
    }
}

/// Either kind of person, chosen per task and run.
pub enum Human {
    Scripted(ScriptedHuman),
    Simulated(Box<SimulatedHuman>),
}

impl Human {
    /// Tokens the person's model used; none for a script.
    pub fn usage(&self) -> TokenUsage {
        match self {
            Human::Scripted(_) => TokenUsage::default(),
            Human::Simulated(h) => h.usage,
        }
    }
}

impl Person for Human {
    async fn reply(&mut self, transcript: &Transcript) -> Result<Reply> {
        match self {
            Human::Scripted(h) => h.reply(transcript).await,
            Human::Simulated(h) => h.reply(transcript).await,
        }
    }
}
