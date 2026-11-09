# `MOJO_VERSION` and `MojoVersion()` read the language version the
# compilation follows, at compile time and at run time: the pin's 1.3.0.
from std.sys.defines import MOJO_VERSION, MojoVersion


def main():
    print(MOJO_VERSION.major, MOJO_VERSION.minor, MOJO_VERSION.patch)
    comptime if MOJO_VERSION.major >= 1:
        print("major at least 1")
    comptime version = MojoVersion()
    print(version.minor)
    var copy = MojoVersion()
    print(copy.patch)
