// Intentional fixture cycle: b -> a -> b
import { a } from './a';
export const b = () => a();
