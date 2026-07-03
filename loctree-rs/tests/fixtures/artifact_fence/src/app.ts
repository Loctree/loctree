// Product code: emits a real runtime event with no test coverage.
import { emit } from '@tauri-apps/api/event';

export async function saveUser(name: string): Promise<void> {
  await emit('user_saved', { name });
}
