# `comptime for` over a compile-time tuple of strings, and over a compile-time
# list of ints (data-driven unrolling). A ledgered divergence: the pinned Mojo
# rejects iterating a Tuple ("does not implement the '__iter__' method"), so
# `assets/ok/comptime_tuple.mojo` iterates a list instead.
comptime states = ("empty", "occupied", "deleted")
comptime sizes = [2, 4, 8]

def main():
    comptime for state in states:
        print(state)
    comptime for n in sizes:
        print(n * n)
