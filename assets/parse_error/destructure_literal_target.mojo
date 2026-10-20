# expect: invalid assignment target
# A literal is not an assignment target: destructuring distributes a value
# to places and declarations, it does not compare.
def main():
    var p = (1, 2)
    (var a, 0) = p
    print(a)
