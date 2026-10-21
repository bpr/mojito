# expect: conflicts with live reference
# A starred remainder borrows its list: appending to the list before the
# remainder's last use is rejected.
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var [first, *rest] = values
    values.append(6)
    print(first, len(rest))
