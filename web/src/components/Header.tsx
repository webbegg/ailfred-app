/// The page's header: New conversation, as Cmd+N in the chat (off until the page knows the
/// conversation's thread).
export function Header({ connected, onNew }: { connected: boolean; onNew: () => void }) {
  return (
    <header className="header">
      <span className="title">Ailfred</span>
      <button type="button" className="new-conversation" disabled={!connected} onClick={onNew}>
        New conversation
      </button>
    </header>
  );
}
