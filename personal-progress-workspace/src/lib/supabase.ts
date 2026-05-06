import { createClient } from "@supabase/supabase-js";
import { env } from "./env";

const fallbackSupabaseUrl = "https://example.supabase.co";
const fallbackSupabaseAnonKey = "anon-key-placeholder";

export const supabase = createClient(
  env.hasSupabaseConfig ? env.supabaseUrl : fallbackSupabaseUrl,
  env.hasSupabaseConfig ? env.supabaseAnonKey : fallbackSupabaseAnonKey,
  {
    auth: {
      persistSession: true,
      autoRefreshToken: true,
      detectSessionInUrl: true,
    },
  },
);
