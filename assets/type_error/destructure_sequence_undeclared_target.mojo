# expect: undeclared
# A bare name in a sequence target must already be declared.
def main() raises:
    var values: List[Int] = [1, 2]
    [p, q] = values
    print(p, q)
