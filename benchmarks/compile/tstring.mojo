# A comptime-dependent generic application inside a t-string interpolation
# monomorphizes through the desugar's part recursion.
def tag[n: Int]() -> Int:
    comptime if n > 2:
        return n * 10
    else:
        return n

def main():
    print(t"big={tag[5]()} small={tag[1]()}")
