# A reference returned through a call writes back to the referent the
# caller owns; the surrounding `try` never fires.
def borrow(ref item: Int) -> ref[item] Int:
    return item


def main():
    var value = 40
    try:
        ref borrowed = borrow(value)
        borrowed += 2
    except error:
        print(error)
    print(value)
