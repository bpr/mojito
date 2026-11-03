"""IO constants and functions: the standard streams as `RawFileDescriptor`s."""

from std.io.file_descriptor import RawFileDescriptor

comptime stdin = RawFileDescriptor(0)
comptime stdout = RawFileDescriptor(1)
comptime stderr = RawFileDescriptor(2)
