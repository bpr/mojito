# expect: use of invalidated interior reference
# requires: stdlib
# A case's starred remainder borrows the subject: appending to it in the
# body before the remainder's last use is rejected.
def main():
    var values: List[Int] = [1, 2, 3, 4, 5]
    match values:
        case [first, *rest]:
            values.append(6)
            print(first, len(rest))
        case _:
            print("no")
