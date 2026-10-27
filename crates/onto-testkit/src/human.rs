//! The person in a conversation (feature `llm`).
//!
//! Either a [`ScriptedHuman`] with fixed replies, or a [`SimulatedHuman`]: a
//! second model playing the person from a task's [`Persona`]. The simulated
//! person sees only what a person would see (the messages, not tool calls),
//! knows only what the persona says, and decides when it is satisfied and
//! whether to accept a proposal. One model plays the agent, another the person.

use anyhow::Result;
use serde::Deserialize;

use crate::agent::{Reply, ScriptedHuman};
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

/// A model playing the person.
pub struct SimulatedHuman {
    persona: Persona,
    model: Agent,
    replies: usize,
    /// Who accepts proposals, recorded on the acceptance.
    pub accepted_by: String,
}

impl SimulatedHuman {
    pub fn new(persona: Persona, model: Agent) -> Self {
        Self { persona, model, replies: 0, accepted_by: "simulated-person".into() }
    }
}

/// The conversation as the person saw it: messages and their own actions, no tool calls.
pub fn person_view(transcript: &Transcript) -> String {
    transcript
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Human { text } => Some(format!("YOU: {text}")),
            Event::Agent { text } => Some(format!("ASSISTANT: {text}")),
            Event::HumanAction { action, detail } => Some(format!("(you did: {action} {detail})")),
            Event::ToolCall { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
        let prompt = format!("The conversation so far:\n\n{}\n\nWrite your next message to the assistant, or DONE.", person_view(transcript));
        let text = ask(&self.model, &self.persona.system_prompt(), &prompt).await?;
        Ok(parse_reply(&text, self.persona.accepts_proposals, &self.accepted_by))
    }
}

/// Either kind of person, chosen per task and run.
pub enum Human {
    Scripted(ScriptedHuman),
    Simulated(Box<SimulatedHuman>),
}

impl Person for Human {
    async fn reply(&mut self, transcript: &Transcript) -> Result<Reply> {
        match self {
            Human::Scripted(h) => h.reply(transcript).await,
            Human::Simulated(h) => h.reply(transcript).await,
        }
    }
}
