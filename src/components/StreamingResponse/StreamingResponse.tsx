import { memo } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openExternal } from "../../services/halo";

// Raw HTML in model output is never rendered (`skipHtml`), remote images
// are not loaded, and links open in the system browser only on click.
const components: Components = {
  a: ({ href, children }) => (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        if (href && /^https?:\/\//i.test(href)) void openExternal(href);
      }}
      title={href}
    >
      {children}
    </a>
  ),
  img: ({ alt }) => <span className="md-image-alt">[image: {alt ?? "untitled"}]</span>,
};

interface Props {
  text: string;
  streaming: boolean;
}

export const StreamingResponse = memo(function StreamingResponse({ text, streaming }: Props) {
  return (
    <div className="markdown" aria-live="polite" aria-busy={streaming}>
      <Markdown remarkPlugins={[remarkGfm]} skipHtml components={components}>
        {text}
      </Markdown>
      {streaming && <span className="stream-cursor" data-testid="stream-cursor" aria-hidden />}
    </div>
  );
});
