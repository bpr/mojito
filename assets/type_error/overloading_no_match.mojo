# A call whose argument type no candidate accepts is rejected, not coerced.
# expect: no overload matches
def f(x: Int) -> Int:
    return x

def f(x: String) -> Int:
    return x.byte_length()

def main():
    var r: Int = f(True)
