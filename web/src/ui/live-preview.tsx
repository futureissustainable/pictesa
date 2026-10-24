import { useState } from "react";

/** The newest in-progress frame of a run, fading in over the one before it. */
export function LivePreview({ src }: { src: string }) {
  // The previous frame stays underneath until the new one has faded in.
  const [frames, setFrames] = useState([src]);
  if (frames[frames.length - 1] !== src) setFrames([...frames.slice(-1), src]);
  return (
    <span className="live-preview" aria-hidden="true">
      {frames.map((frame) => (
        <img key={frame} src={frame} alt="" />
      ))}
    </span>
  );
}
