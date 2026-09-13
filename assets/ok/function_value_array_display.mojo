# A display of non-capturing function values is a fixed-size array of that
# thin function type; each element calls through its subscript.
def double(x: Int) -> Int:
    return x * 2

def triple(x: Int) -> Int:
    return x * 3

def main():
    var fns = [double, triple]
    print(len(fns))
    print(fns[0](5), fns[1](5))
