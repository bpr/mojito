# expect: use of uninitialized value 'values'
# A starred remainder is a view of its list's owned elements: moving the
# list before the remainder's last use leaves the remainder reading an
# uninitialized value, as the pin reports it.
# requires: stdlib
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var [first, *rest] = values
    var moved = values^
    print(first, len(rest), len(moved))
