const std = @import("std");

pub const VERSION = "0.9.0";
pub var call_count: u32 = 0;

pub fn greet(name: []const u8) !void {
    call_count += 1;
    const stdout = std.io.getStdOut().writer();
    try stdout.print("hello, {s}!\n", .{name});
}

test "greet increments call_count" {
    try greet("test");
    try std.testing.expect(call_count >= 1);
}
