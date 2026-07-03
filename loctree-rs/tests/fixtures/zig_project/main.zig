const std = @import("std");
const helpers = @import("helpers.zig");

pub const APP_NAME = "loctree-fixture";

pub fn main() !void {
    const stdout = std.io.getStdOut().writer();
    try stdout.print("{s} v{s}\n", .{ APP_NAME, helpers.VERSION });
    try helpers.greet("world");
}

test "main produces some output" {
    try std.testing.expect(APP_NAME.len > 0);
}
