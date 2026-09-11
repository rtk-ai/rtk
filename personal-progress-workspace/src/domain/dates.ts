import { format } from "date-fns";

export function todayIsoDate(now = new Date()): string {
  return format(now, "yyyy-MM-dd");
}
