"""IO constants and functions: the standard streams as `FileDescriptor`s."""

from std.io import FileDescriptor

comptime stdin = FileDescriptor(0)
comptime stdout = FileDescriptor(1)
comptime stderr = FileDescriptor(2)
