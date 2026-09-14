# expect: 'Tuple[String, String, String]' does not implement the '__iter__' method
# `comptime for` cannot iterate a compile-time Tuple, which has no `__iter__`,
# as upstream; `assets/ok/comptime_tuple.mojo` iterates a compile-time list.
comptime states = ("empty", "occupied", "deleted")

def main():
    comptime for state in states:
        print(state)
