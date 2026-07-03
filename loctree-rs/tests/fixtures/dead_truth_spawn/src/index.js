import { spawn } from 'child_process';

export function start() {
  return spawn('voice-daemon');
}
