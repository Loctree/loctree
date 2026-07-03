// Date utilities
export function formatDate(date: Date): string {
    return date.toISOString();
}

export function parseDate(str: string): Date {
    return new Date(str);
}
