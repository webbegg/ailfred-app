// Ailfred's pi extension while computer control is on: every action of its tools returns a
// fresh screenshot, and pi would send all of them to its model at each step, so the context
// and the cost would grow with every one. Before each model call this keeps the images of the
// latest computer tool result that has any, and puts a short note where each earlier one was.
// The conversation pi keeps is untouched: only what goes to the model changes.
// Written to pi's agent folder by `pi_setup::computer_tools`; plain JavaScript, no imports.

const COMPUTER_TOOL = "mcp__computer__";
const LEFT_OUT = "[An earlier screenshot, left out: only the latest is kept.]";

function isComputerScreenshot(message) {
  return (
    message?.role === "toolResult" &&
    typeof message.toolName === "string" &&
    message.toolName.startsWith(COMPUTER_TOOL) &&
    Array.isArray(message.content) &&
    message.content.some((part) => part?.type === "image")
  );
}

// The messages with only the latest computer screenshot left in, or undefined when there's
// nothing to leave out (so pi keeps the list it has, and its cached prefix).
function keepLatestScreenshot(messages) {
  let latest = -1;
  for (let i = messages.length - 1; i >= 0; i--) {
    if (isComputerScreenshot(messages[i])) {
      latest = i;
      break;
    }
  }
  let changed = false;
  const kept = messages.map((message, i) => {
    if (i >= latest || !isComputerScreenshot(message)) return message;
    changed = true;
    const content = message.content.map((part) => (part?.type === "image" ? { type: "text", text: LEFT_OUT } : part));
    return { ...message, content };
  });
  return changed ? kept : undefined;
}

export default function (pi) {
  pi.on("context", async (event) => {
    const messages = keepLatestScreenshot(event.messages);
    return messages ? { messages } : undefined;
  });
}
