# expect: invalid assignment target
# A literal is not a sequence destructuring target.
def main() raises:
    var values: List[Int] = [1, 2]
    var b = 0
    [1, b] = values
    print(b)
