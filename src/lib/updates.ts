import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type UpdateStatus } from "./api";

/** The app's update status, kept current as checks and downloads happen. */
export function useUpdates(): UpdateStatus | null {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  useEffect(() => {
    api.updateStatus().then(setStatus).catch(() => {});
    const off = listen<UpdateStatus>("update-status", (e) => setStatus(e.payload));
    return () => void off.then((f) => f());
  }, []);
  return status;
}
