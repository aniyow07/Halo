import type { ProcessingStats } from "../../types";

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

interface Props {
  src: string | null;
  stats: ProcessingStats | null;
  processing: boolean;
}

export function ImagePreview({ src, stats, processing }: Props) {
  return (
    <div className="image-preview">
      {src ? <img src={src} alt="Selected region" /> : <div className="image-placeholder">{processing ? "Processing…" : ""}</div>}
      {stats && (
        <p className="image-stats" data-testid="image-stats">
          {stats.processedWidth}×{stats.processedHeight} {stats.format.toUpperCase()} · {formatBytes(stats.processedBytes)}
          {stats.originalBytes > 0 && (
            <>
              {" "}(was {formatBytes(stats.originalBytes)} PNG
              {stats.originalWidth !== stats.processedWidth ? ` at ${stats.originalWidth}×${stats.originalHeight}` : ""},{" "}
              {stats.reductionPercent >= 0 ? `−${stats.reductionPercent}%` : `+${Math.abs(stats.reductionPercent)}%`})
            </>
          )}
        </p>
      )}
    </div>
  );
}
