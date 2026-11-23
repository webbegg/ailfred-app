import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openable } from "../transport";

const PLUGINS = [remarkGfm];

/// The model's text as Markdown. Raw HTML in it is skipped; a link opens outside the page (the
/// page never navigates); an image shows only from a data URL, the one kind the page may load.
export function Markdown({ text, onOpen }: { text: string; onOpen: (url: string) => void }) {
  const components: Components = {
    a: ({ href, children }) =>
      href && openable(href) ? (
        <a
          href={href}
          title={href}
          onClick={(e) => {
            e.preventDefault();
            onOpen(href);
          }}
        >
          {children}
        </a>
      ) : (
        <span className="link">{children}</span>
      ),
    img: ({ src, alt }) => (typeof src === "string" && src.startsWith("data:image/") ? <img src={src} alt={alt ?? ""} /> : <span className="image-alt">{alt}</span>),
  };
  return (
    <div className="markdown">
      <ReactMarkdown remarkPlugins={PLUGINS} skipHtml components={components} urlTransform={(url) => (url.startsWith("data:image/") || openable(url) ? url : "")}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
