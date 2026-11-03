# A capturing lambda's value is its closure's storage, a run-time value, so
# it cannot bind a compile-time callable parameter.
# expect: cannot use a capturing lambda in type parameter
def observe[f: def(x: Int) thin -> Int](value: Int) -> Int:
    return f(value)

def main():
    var factor = 3
    print(observe[lambda (x: Int) -> Int: x * factor](5))
