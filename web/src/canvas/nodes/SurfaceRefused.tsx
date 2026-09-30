export function SurfaceRefused({ text }: { text: string | null }) {
  if (text === null) {
    return null;
  }
  return (
    <span
      role="status"
      title="The server refused this plot"
      className="absolute inset-x-0 top-1/2 -translate-y-1/2 px-2 text-center text-xs text-danger"
    >
      {text}
    </span>
  );
}
