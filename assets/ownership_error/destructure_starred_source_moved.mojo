# expect: conflicts with live reference
# A starred remainder borrows its list: moving the list before the
# remainder's last use is rejected.
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var [first, *rest] = values
    var moved = values^
    print(first, len(rest), len(moved))
