# expect: invalidated interior reference
# The inner iteration's element borrow roots at the outer container through
# the `ref` row binding; appending to the row reallocates that storage, so the
# later use of the yielded element is rejected even though the mutation and
# the borrow both execute through the binding rather than the owner.
def main():
    var rows: List[List[Int]] = [[1, 2], [3, 4]]
    for ref row in rows:
        for x in row:
            row.append(9)
            print(x)
