# expect: invalidated interior reference
# Structurally mutating a List while a comprehension's borrowing iterator holds
# its `element` interior generation invalidates the iterator before its next
# use — the same interior loan granularity a `for` statement enforces.
def first(mut xs: List[Int]) -> Int:
    xs.append(9)
    return 0

def main():
    var values = [1, 2, 3]
    var doubled = [x + first(values) for x in values]
    print(len(doubled))
