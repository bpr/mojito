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
