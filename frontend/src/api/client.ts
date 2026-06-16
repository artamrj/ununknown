export const api = async <T>(path: string, init?: RequestInit): Promise<T> => {
  const response = await fetch(`/api${path}`, {
    headers: { "Content-Type": "application/json" },
    ...init,
  });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || response.statusText);
  return body;
};
