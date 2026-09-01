# `List.__add__` takes `var other`: the operand transfers or copies.
def main():
    var p: List[Int] = [1]
    var q: List[Int] = [2, 3]
    var joined = p + q.copy()
    print(len(joined), len(q))
    var moved = joined + q^
    print(len(moved))
