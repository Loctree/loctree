// Module C - imports from A (completes the cycle!)
import { funcA } from './a';

export function funcC(): string {
    return 'C calls ' + funcA();
}
