import { useState } from "react";
import { answerText } from "../commands";
import type { QuestionPart } from "../state";

/// How the dialogs of Ailfred's guard start (`ailfred_core::reply::LOCAL_ONLY`).
export const LOCAL_ONLY = "Ailfred guard: ";

/// A question the model asked. Open, its options are buttons (one answers at once; with `multi`
/// they toggle and Send answers), Other takes an answer of the user's own, and Decline closes it
/// unanswered; answered or closed, it is read only. Ailfred's guard's question (`LOCAL_ONLY`)
/// approves an action on the user's accounts or computer: only Ailfred's own window answers it
/// (the server refuses an answer from here), so it offers Decline only.
export function QuestionView({ part, open, onAnswer, onDecline }: { part: QuestionPart; open: boolean; onAnswer: (answer: string) => void; onDecline: () => void }) {
  const [picked, setPicked] = useState<number[]>([]);
  const [other, setOther] = useState("");
  if (!open) {
    return (
      <div className="question closed">
        <p className="prompt">{part.prompt}</p>
        {part.options.length > 0 && (
          <ul className="options">
            {part.options.map((o, i) => (
              <li key={i} className={part.answered?.split(", ").includes(o) ? "chosen" : undefined}>
                {o}
              </li>
            ))}
          </ul>
        )}
        <p className="answered">{part.answered !== null ? <>Answered: {part.answered}</> : "Not answered."}</p>
      </div>
    );
  }
  if (part.prompt.startsWith(LOCAL_ONLY)) {
    return (
      <div className="question">
        <p className="prompt">{part.prompt}</p>
        <p className="local-only">Answer in Ailfred's window.</p>
        <button type="button" className="decline" onClick={onDecline}>
          Decline
        </button>
      </div>
    );
  }
  const answer = answerText(part.options, picked, other);
  const pick = (i: number) => {
    if (!part.multi) return onAnswer(part.options[i]);
    setPicked((p) => (p.includes(i) ? p.filter((x) => x !== i) : [...p, i]));
  };
  return (
    <div className="question">
      <p className="prompt">{part.prompt}</p>
      <div className="options">
        {part.options.map((o, i) => (
          <button key={i} type="button" className={picked.includes(i) ? "option picked" : "option"} aria-pressed={part.multi ? picked.includes(i) : undefined} onClick={() => pick(i)}>
            {o}
          </button>
        ))}
      </div>
      <form
        className="other"
        onSubmit={(e) => {
          e.preventDefault();
          if (answer !== "") onAnswer(answer);
        }}
      >
        <input type="text" placeholder="Other…" value={other} onChange={(e) => setOther(e.target.value)} />
        {(part.multi || other.trim() !== "") && (
          <button type="submit" className="answer-send" disabled={answer === ""}>
            Send
          </button>
        )}
        <button type="button" className="decline" onClick={onDecline}>
          Decline
        </button>
      </form>
    </div>
  );
}
