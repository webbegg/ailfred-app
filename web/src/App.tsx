import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, useState } from "react";
import { answerCommand, declineCommand, newConversationCommand, newRunId, retryCommand, sendCommand, stopCommand, type Image } from "./commands";
import { ConversationView } from "./components/Conversation";
import { Composer } from "./components/Composer";
import { Header } from "./components/Header";
import type { Answer, Command } from "./protocol";
import { applyAll, empty, local, type Conversation, type Local, type QuestionPart, type ReplyTurn } from "./state";
import type { Transport } from "./transport";

type Action = { type: "events"; events: unknown[] } | Local;

function reducer(state: Conversation, action: Action): Conversation {
  return action.type === "events" ? applyAll(state, action.events) : local(state, action);
}

/// Why a command was refused, from the app's answer.
function refusal(answer: Answer): string {
  return typeof answer.body.error === "string" && answer.body.error !== "" ? answer.body.error : `Refused (${answer.status}).`;
}

/// How close to the bottom counts as at it, in pixels.
const STICK = 48;

export function App({ transport }: { transport: Transport }) {
  const [state, dispatch] = useReducer(reducer, undefined, empty);
  const [error, setError] = useState<string | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const started = useRef(false);

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    transport.start((events) => dispatch({ type: "events", events }));
  }, [transport]);

  // Follow the reply to the bottom, unless the user scrolled up.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [state]);

  const onScroll = () => {
    const el = scroller.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < STICK;
  };

  const onSend = useCallback(
    async (text: string, images: Image[]) => {
      if (state.threadId === null) {
        setError("Not connected to Ailfred's conversation yet.");
        return false;
      }
      const runId = newRunId();
      dispatch({ type: "sent", runId, text, images: images.map((i) => `data:${i.mimeType};base64,${i.data}`) });
      stick.current = true;
      const answer = await transport.send(sendCommand(state.threadId, runId, text, images));
      if (answer.status === 202) {
        setError(null);
        return true;
      }
      dispatch({ type: "refused", runId });
      setError(refusal(answer));
      return false;
    },
    [state.threadId, transport],
  );

  const onStop = useCallback(async () => {
    if (state.threadId === null || state.run === null) return;
    const answer = await transport.send(stopCommand(state.threadId, state.run));
    setError(answer.status === 202 ? null : refusal(answer));
  }, [state.threadId, state.run, transport]);

  const onAnswer = useCallback(
    async (_reply: ReplyTurn, question: QuestionPart, text: string) => {
      if (state.threadId === null) return;
      const answer = await transport.send(answerCommand(state.threadId, newRunId(), question.interruptId, text));
      if (answer.status === 202) {
        dispatch({ type: "answered", interruptId: question.interruptId, answer: text });
        setError(null);
      } else setError(refusal(answer));
    },
    [state.threadId, transport],
  );

  // The commands whose effect comes back on the stream (a snapshot, or a run): only a refusal
  // shows here.
  const command = useCallback(
    async (build: (threadId: string) => Command) => {
      if (state.threadId === null) return;
      const answer = await transport.send(build(state.threadId));
      setError(answer.status === 202 ? null : refusal(answer));
    },
    [state.threadId, transport],
  );

  const onDecline = useCallback((question: QuestionPart) => command((t) => declineCommand(t, newRunId(), question.interruptId)), [command]);
  const onRetry = useCallback(
    (reply: ReplyTurn) => {
      stick.current = true;
      return command((t) => retryCommand(t, newRunId(), reply.runId));
    },
    [command],
  );
  const onNew = useCallback(() => command(newConversationCommand), [command]);

  const onOpen = useCallback((url: string) => transport.open(url), [transport]);

  return (
    <div className="app">
      <Header connected={state.threadId !== null} onNew={onNew} />
      <div className="scroller" ref={scroller} onScroll={onScroll}>
        {state.items.length === 0 && state.pending.length === 0 ? (
          <p className="placeholder">{state.threadId === null ? "Connecting…" : "Ask Ailfred anything."}</p>
        ) : (
          <ConversationView state={state} onOpen={onOpen} onAnswer={onAnswer} onDecline={onDecline} onRetry={onRetry} />
        )}
      </div>
      <Composer streaming={state.run !== null} error={error} onSend={onSend} onStop={onStop} />
    </div>
  );
}
