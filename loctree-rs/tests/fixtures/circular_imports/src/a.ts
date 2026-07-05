// Module A - imports from B (circular!)
import { funcB } from './b';

export function funcA(): string {
    return 'A calls ' + funcB();
}
