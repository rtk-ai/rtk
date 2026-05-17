const estimateError = "Use minutes or hours, for example 30 mins or 1.5h.";

export interface ParsedEstimateTime {
  minutes: number | null;
  error: string | null;
}

export function parseEstimateTimeInput(value: string): ParsedEstimateTime {
  const normalized = value.trim().toLowerCase();

  if (!normalized) {
    return { minutes: null, error: null };
  }

  const minutesMatch = normalized.match(/^(\d+)\s*(m|min|mins|minute|minutes)?$/);
  if (minutesMatch) {
    return { minutes: Number(minutesMatch[1]), error: null };
  }

  const hoursMatch = normalized.match(/^(\d+(?:\.\d+)?)\s*(h|hr|hrs|hour|hours)$/);
  if (hoursMatch) {
    return { minutes: Math.round(Number(hoursMatch[1]) * 60), error: null };
  }

  return { minutes: null, error: estimateError };
}
