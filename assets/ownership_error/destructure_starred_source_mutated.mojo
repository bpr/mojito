# expect: use of invalidated interior reference 'rest' to 'values["element"]'
# A starred remainder is a `Span` over its list's owned elements: appending
# to the list stales that interior, and the remainder's next use is
# rejected, as at the pin.
# requires: stdlib
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var [first, *rest] = values
    values.append(6)
    print(first, len(rest))
