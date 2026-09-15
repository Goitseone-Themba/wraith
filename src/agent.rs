//! Opt-in project-local opencode agent + tool files that give the voice agent
//! a speech persona, permission to ask questions, and a `wraith_say` bridge.

use std::fs;
use std::path::{Path, PathBuf};

pub const MARKER: &str = "managed-by: wraith";

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentStatus {
    pub installed: bool,
    pub agent_path: String,
    pub tool_path: String,
    pub agent_managed: bool,
    pub tool_managed: bool,
}

pub const AGENT_CONTENT: &str = r#"---
description: Voice-first agent for Wraith realtime calls
mode: primary
temperature: 0.4
permission:
  question: allow
---
<!-- managed-by: wraith -->
You are speaking with the user over a realtime voice call.

- Keep replies short and conversational, like a colleague on the phone.
- Never read code, diffs, file paths or URLs aloud. Summarize them and keep the
  details on screen.
- Before long-running tool work, say what you are about to do in one sentence.
- Use the `question` tool when a decision is genuinely needed; the user answers
  by voice.
- If the `wraith_say` tool is available, use it for brief milestone updates
  while you work.
"#;

pub const TOOL_CONTENT: &str = r#"// managed-by: wraith
import { tool } from "@opencode-ai/plugin"

export const worker = tool({
  description: "Spawn a background opencode worker session for a longer task. The worker runs in parallel; its session appears in Wraith's Workers panel. Use this only when the user asks for substantial parallel work.",
  args: {
    prompt: tool.schema.string().describe("Complete task description for the worker"),
    title: tool.schema.string().optional().describe("Short title for the worker"),
  },
  async execute(args) {
    const url = process.env.WRAITH_BRIDGE_URL
    const token = process.env.WRAITH_BRIDGE_TOKEN
    if (!url || !token) return "Wraith voice bridge is not connected."

    try {
      const response = await fetch(`${url}/agent/worker`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-wraith-token": token,
        },
        body: JSON.stringify({ prompt: args.prompt, title: args.title }),
      })
      if (!response.ok) return `Wraith worker bridge returned ${response.status}.`
      const data = await response.json()
      return `Worker started: ${data.worker?.id ?? "unknown"}`
    } catch (error) {
      return `Wraith worker bridge unavailable: ${error}`
    }
  },
})

export const say = tool({
  description: "Speak a short progress update to the user through Wraith's realtime voice call. Use sparingly for milestones the user should hear.",
  args: {
    text: tool.schema.string().describe("One short sentence to speak aloud"),
  },
  async execute(args) {
    const url = process.env.WRAITH_BRIDGE_URL
    const token = process.env.WRAITH_BRIDGE_TOKEN
    if (!url || !token) return "Wraith voice bridge is not connected."

    try {
      const response = await fetch(`${url}/agent/say`, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-wraith-token": token,
        },
        body: JSON.stringify({ text: args.text }),
      })
      return response.ok ? "Spoken." : `Wraith voice bridge returned ${response.status}.`
    } catch (error) {
      return `Wraith voice bridge unavailable: ${error}`
    }
  },
})
"#;

fn agent_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".opencode").join("agents").join("wraith-voice.md")
}

fn tool_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".opencode").join("tools").join("wraith.ts")
}

fn describe(agent: &Path, tool: &Path) -> AgentStatus {
    let agent_text = fs::read_to_string(agent).ok();
    let tool_text = fs::read_to_string(tool).ok();

    let agent_managed = agent_text.as_deref().map(|t| t.contains(MARKER)).unwrap_or(false);
    let tool_managed = tool_text.as_deref().map(|t| t.contains(MARKER)).unwrap_or(false);

    AgentStatus {
        installed: agent_text.is_some() || tool_text.is_some(),
        agent_path: agent.display().to_string(),
        tool_path: tool.display().to_string(),
        agent_managed,
        tool_managed,
    }
}

pub fn status(project_dir: &str) -> AgentStatus {
    let project = PathBuf::from(project_dir);
    describe(&agent_path(&project), &tool_path(&project))
}

pub fn install(project_dir: &str) -> Result<AgentStatus, String> {
    let project = PathBuf::from(project_dir);
    let agent = agent_path(&project);
    let tool = tool_path(&project);

    for path in [&agent, &tool] {
        if let Ok(existing) = fs::read_to_string(path) {
            if !existing.contains(MARKER) {
                return Err(format!(
                    "refusing to overwrite unmanaged file {}",
                    path.display()
                ));
            }
        }
    }

    if let Some(parent) = agent.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    if let Some(parent) = tool.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }

    fs::write(&agent, AGENT_CONTENT).map_err(|e| format!("write {}: {e}", agent.display()))?;
    fs::write(&tool, TOOL_CONTENT).map_err(|e| format!("write {}: {e}", tool.display()))?;

    Ok(describe(&agent, &tool))
}

pub fn remove(project_dir: &str) -> Result<AgentStatus, String> {
    let project = PathBuf::from(project_dir);
    let agent = agent_path(&project);
    let tool = tool_path(&project);

    for path in [&agent, &tool] {
        if let Ok(existing) = fs::read_to_string(path) {
            if existing.contains(MARKER) {
                fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
            }
        }
    }

    Ok(describe(&agent, &tool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_and_remove_roundtrip() {
        let dir = tempdir().expect("tempdir");
        let project = dir.path().display().to_string();

        let status = status(&project);
        assert!(!status.installed);

        let installed = install(&project).expect("install");
        assert!(installed.installed);
        assert!(installed.agent_managed);
        assert!(installed.tool_managed);

        let removed = remove(&project).expect("remove");
        assert!(!removed.installed);
    }

    #[test]
    fn install_refuses_unmanaged_files() {
        let dir = tempdir().expect("tempdir");
        let project = dir.path().display().to_string();
        let agent = agent_path(&PathBuf::from(&project));
        fs::create_dir_all(agent.parent().unwrap()).unwrap();
        fs::write(&agent, "user content").unwrap();

        assert!(install(&project).is_err());
    }
}
