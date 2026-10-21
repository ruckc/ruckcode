/** In-app update types and the API the UI uses (Tauri commands, or a fake in browser mock mode). */

export type UpdateCheck =
  /** No signing key in this build, or no manifest published: updates are off. */
  | { state: 'notConfigured'; reason: string }
  | { state: 'upToDate'; version: string }
  | {
      state: 'available';
      version: string;
      current: string;
      notes: string | null;
      date: string | null;
      /** Why this install cannot update itself (deb/msi/unbundled); null when it can. */
      manualReason: string | null;
      downloadPage: string;
    };

export interface UpdatesApi {
  /** Ask the update site; stages an update the other calls act on. */
  check(): Promise<UpdateCheck>;
  /** Download and verify the staged update; `onProgress` gets bytes so far and the total, if known. */
  download(onProgress: (downloaded: number, total: number | null) => void): Promise<void>;
  /** Apply the downloaded update (Windows: runs the installer and exits the app). */
  install(): Promise<void>;
  /** Relaunch the app. */
  restart(): Promise<void>;
}
