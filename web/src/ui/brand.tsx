import { Link } from "../lib/router";

export const MARK_SRC = "/brand/pictesa-camera-128.png";

export function PixelMark({ className = "" }: { className?: string }) {
  return <img className={`pixel-mark ${className}`} src={MARK_SRC} alt="" aria-hidden="true" width={32} height={32} decoding="async" />;
}

export function Brand({ className = "" }: { className?: string }) {
  return (
    <Link className={`brand ${className}`} to="/" aria-label="Pictesa, create">
      <PixelMark />
      <span>pictesa</span>
    </Link>
  );
}
