export interface Job {
  id: number;
  modem_id: string | null;
  operation: string;
  queued_ms: number;
  elapsed_ms: number | null;
  commands_started: number;
  last_command: string | null;
  caller_detached: boolean | null;
  outcome: string | null;
}
export interface Queue {
  state: string;
  capacity: number;
  waiting_count: number;
  current: Job | null;
  waiting: Job[];
  recent: Job[];
  completed: number;
  failed: number;
  cancelled_before_start: number;
  rejected_queue_full: number;
}
export interface Port {
  path: string;
  roles: string[];
  canonical_path: string | null;
  opened: boolean;
  queue: Queue | null;
}
export interface Modem {
  id: string;
  name: string;
  enabled: boolean;
  manufacturer: string;
  model: string;
  bus: string;
  ports: Port[];
}
export interface Snapshot {
  modems: Modem[];
  history_limit_per_port: number;
  payloads_included: boolean;
}
