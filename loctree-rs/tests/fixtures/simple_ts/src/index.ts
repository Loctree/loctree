// Simple TypeScript project entry point
import { greet } from './utils/greeting';
import { formatDate } from './utils/date';

export function main() {
    console.log(greet('World'));
    console.log(formatDate(new Date()));
}

main();
