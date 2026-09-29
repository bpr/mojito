# expect: the temporary result of call 'value()'
# A call returning a value is no assignment target.
def value(a: Int) -> Int:
    return a


def main():
    var k = 1
    value(k) = 9
    print(k)
