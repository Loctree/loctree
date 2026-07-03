// Module B - imports from C
import { funcC } from './c';

export function funcB(): string {
    return 'B calls ' + funcC();
}
