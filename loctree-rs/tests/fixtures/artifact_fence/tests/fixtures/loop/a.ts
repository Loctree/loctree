// Intentional fixture cycle: a -> b -> a
import { b } from './b';
export const a = () => b();
