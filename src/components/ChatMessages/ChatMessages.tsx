import { useEffect, useRef } from "react";
import type { Message } from "../../app/conversation";
import { ErrorDisplay } from "../ErrorDisplay/ErrorDisplay";
import { StreamingResponse } from "../StreamingResponse/StreamingResponse";

export function ChatMessages({ messages }: { messages: Message[] }) {
  const endRef = useRef<HTMLDivElement>(null);
  const last = messages[messages.length - 1];
  useEffect(() => {
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [messages.length, last?.answer]);

  if (messages.length === 0) return null;
  return (
    <div className="chat-messages" data-testid="chat-messages">
      {messages.map((m) => (
        <div key={m.id} className={`chat-turn status-${m.status}`}>
          <p className="chat-question">{m.question}</p>
          {m.status === "pending" && <p className="chat-status">Waiting for Gemini…</p>}
          {m.answer && <StreamingResponse text={m.answer} streaming={m.status === "streaming"} />}
          {m.status === "cancelled" && <p className="chat-status">Cancelled.</p>}
          {m.status === "done" && m.finishReason && m.finishReason !== "STOP" && (
            <p className="chat-status">Answer ended early ({m.finishReason}).</p>
          )}
          {m.status === "error" && m.error && <ErrorDisplay error={m.error} />}
        </div>
      ))}
      <div ref={endRef} />
    </div>
  );
}
