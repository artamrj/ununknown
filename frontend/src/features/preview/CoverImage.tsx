import { useState } from "react";

export function CoverImage({ src, title }: { src?: string; title?: string }) {
  const [failed, setFailed] = useState(false);
  return (
    <div className="cover">
      {src && !failed ? (
        <img src={src} alt="" onError={() => setFailed(true)} loading="lazy" />
      ) : (
        <span>{(title || "?").slice(0, 1)}</span>
      )}
    </div>
  );
}
