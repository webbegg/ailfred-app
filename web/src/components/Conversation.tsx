import type { Conversation, Part, QuestionPart, ReplyTurn, Step, UserTurn } from "../state";
import { canAnswer } from "../state";
import { Markdown } from "./Markdown";
import { QuestionView } from "./Question";

interface Props {
  state: Conversation;
  onOpen: (url: string) => void;
  onAnswer: (reply: ReplyTurn, question: QuestionPart, answer: string) => void;
}

/// The conversation: each user message and its reply, then the messages sent whose run hasn't
/// started yet.
export function ConversationView({ state, onOpen, onAnswer }: Props) {
  return (
    <div className="conversation">
      {state.items.map((item, i) =>
        item.kind === "user" ? (
          <UserView key={`u${i}:${item.id}`} turn={item} />
        ) : (
          <ReplyView key={`r${i}:${item.runId}`} reply={item} onOpen={onOpen} open={(q) => canAnswer(state, item, q)} onAnswer={(q, a) => onAnswer(item, q, a)} />
        ),
      )}
      {state.pending.map((turn) => (
        <UserView key={`p:${turn.id}`} turn={turn} pending />
      ))}
    </div>
  );
}

function UserView({ turn, pending }: { turn: UserTurn; pending?: boolean }) {
  if (turn.text === "" && turn.images.length === 0) return null;
  return (
    <div className={pending ? "user pending" : "user"}>
      <div className="bubble">
        {turn.images.length > 0 && (
          <div className="images">
            {turn.images.map((src, i) => (
              <img key={i} src={src} alt="" />
            ))}
          </div>
        )}
        {turn.text !== "" && <p>{turn.text}</p>}
      </div>
    </div>
  );
}

function ReplyView({ reply, onOpen, open, onAnswer }: { reply: ReplyTurn; onOpen: (url: string) => void; open: (q: QuestionPart) => boolean; onAnswer: (q: QuestionPart, answer: string) => void }) {
  return (
    <div className={`reply ${reply.state}`}>
      {reply.parts.map((part, i) => (
        <PartView key={i} part={part} onOpen={onOpen} open={open} onAnswer={onAnswer} />
      ))}
      {reply.state === "streaming" && <div className="typing" aria-label="Streaming" />}
      {reply.state === "stopped" && <p className="ended">{reply.interrupted ? "Interrupted: the model's process ended." : "Stopped."}</p>}
    </div>
  );
}

function PartView({ part, onOpen, open, onAnswer }: { part: Part; onOpen: (url: string) => void; open: (q: QuestionPart) => boolean; onAnswer: (q: QuestionPart, answer: string) => void }) {
  switch (part.kind) {
    case "text":
      return <Markdown text={part.text} onOpen={onOpen} />;
    case "thinking":
      return (
        <details className="thinking">
          <summary>{part.done ? "Thought" : "Thinking…"}</summary>
          <p>{part.text}</p>
        </details>
      );
    case "steps":
      return <StepsView steps={part.steps} />;
    case "diff":
      return <DiffView path={part.path} patch={part.patch} />;
    case "question":
      return <QuestionView part={part} open={open(part)} onAnswer={(a) => onAnswer(part, a)} />;
    case "error":
      return (
        <p className="error" role="alert">
          {part.message}
        </p>
      );
  }
}

const VERBS: Record<string, [doing: string, did: string, one: string, many: string]> = {
  read: ["Reading", "Read", "file", "files"],
  search: ["Searching", "Searched", "search", "searches"],
  run: ["Running", "Ran", "command", "commands"],
  edit: ["Editing", "Edited", "file", "files"],
  other: ["Using", "Used", "tool", "tools"],
};
const kindOf = (name: string) => (name in VERBS ? name : "other");
const verbs = (name: string) => VERBS[kindOf(name)];

/// One line for steps in a row: what runs now, else what they did (`Agent::summary`).
export function stepsSummary(steps: Step[]): string {
  const running = steps.findLast((s) => s.status === "running");
  if (running) return `${verbs(running.name)[0]} ${running.target || "a tool"}…`;
  const counts = new Map<string, number>();
  for (const s of steps) counts.set(kindOf(s.name), (counts.get(kindOf(s.name)) ?? 0) + 1);
  const parts = [...counts].map(([kind, n]) => {
    const [, did, one, many] = VERBS[kind];
    return `${did} ${n} ${n === 1 ? one : many}`;
  });
  const failed = steps.filter((s) => s.status === "failed").length;
  if (failed > 0) parts.push(`${failed} failed`);
  return parts.join(" · ");
}

function StepsView({ steps }: { steps: Step[] }) {
  const running = steps.some((s) => s.status === "running");
  return (
    <details className="steps" open={running}>
      <summary>{stepsSummary(steps)}</summary>
      <ul>
        {steps.map((s) => (
          <li key={s.id} className={`step ${s.status}`}>
            <details>
              <summary>
                <span className="status" aria-label={s.status}>
                  {s.status === "running" ? "◌" : s.status === "done" ? "✓" : "✕"}
                </span>
                <span className="tool">{s.name}</span>
                <code className="target">{s.target}</code>
                {s.status === "failed" && <span className="badge">failed</span>}
              </summary>
              {s.output !== "" ? <pre className="output">{s.output}</pre> : <p className="no-output">No output.</p>}
            </details>
          </li>
        ))}
      </ul>
    </details>
  );
}

function DiffView({ path, patch }: { path: string; patch: string }) {
  const lines = patch.replace(/\n$/, "").split("\n");
  const added = lines.filter((l) => l.startsWith("+") && !l.startsWith("+++")).length;
  const removed = lines.filter((l) => l.startsWith("-") && !l.startsWith("---")).length;
  return (
    <details className="diff" open={added + removed <= 12}>
      <summary>
        <code>{path}</code> <span className="added">+{added}</span> <span className="removed">−{removed}</span>
      </summary>
      <pre>
        {lines.map((l, i) => (
          <span key={i} className={l.startsWith("+") && !l.startsWith("+++") ? "add" : l.startsWith("-") && !l.startsWith("---") ? "del" : l.startsWith("@@") ? "hunk" : undefined}>
            {l}
            {"\n"}
          </span>
        ))}
      </pre>
    </details>
  );
}
