// managed-by: wraith
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
