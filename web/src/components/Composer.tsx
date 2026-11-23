import { useState, type ClipboardEvent, type KeyboardEvent } from "react";
import type { Image } from "../commands";

interface Props {
  /// A run streams: Stop shows.
  streaming: boolean;
  /// Why the last command was refused.
  error: string | null;
  /// Sends a message; resolves false if it was refused.
  onSend: (text: string, images: Image[]) => Promise<boolean>;
  onStop: () => void;
}

/// Where the user writes: Enter sends, Shift+Enter makes a new line, a pasted PNG goes with the
/// message.
export function Composer({ streaming, error, onSend, onStop }: Props) {
  const [text, setText] = useState("");
  const [images, setImages] = useState<Image[]>([]);
  const empty = text.trim() === "" && images.length === 0;

  const send = async () => {
    if (empty) return;
    const [sentText, sentImages] = [text, images];
    setText("");
    setImages([]);
    if (!(await onSend(sentText, sentImages))) {
      // Refused: what was written comes back, unless something new was.
      setText((t) => (t === "" ? sentText : t));
      setImages((i) => (i.length === 0 ? sentImages : i));
    }
  };

  const keyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void send();
    }
  };

  const paste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const pngs = Array.from(e.clipboardData.files).filter((f) => f.type === "image/png");
    if (pngs.length === 0) return;
    e.preventDefault();
    for (const file of pngs) {
      const reader = new FileReader();
      reader.onload = () => {
        const url = String(reader.result);
        const data = url.slice(url.indexOf(",") + 1);
        setImages((i) => [...i, { mimeType: "image/png", data }]);
      };
      reader.readAsDataURL(file);
    }
  };

  return (
    <div className="composer">
      {error !== null && (
        <p className="refused" role="alert">
          {error}
        </p>
      )}
      {images.length > 0 && (
        <div className="attached">
          {images.map((img, i) => (
            <span key={i} className="thumb">
              <img src={`data:${img.mimeType};base64,${img.data}`} alt="" />
              <button type="button" aria-label="Remove image" onClick={() => setImages((all) => all.filter((_, k) => k !== i))}>
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      <div className="row">
        <textarea value={text} rows={1} placeholder="Message Ailfred" onChange={(e) => setText(e.target.value)} onKeyDown={keyDown} onPaste={paste} />
        {streaming && (
          <button type="button" className="stop" onClick={onStop}>
            Stop
          </button>
        )}
        <button type="button" className="send" disabled={empty} onClick={() => void send()}>
          Send
        </button>
      </div>
    </div>
  );
}
