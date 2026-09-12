def borrow(ref value: Int) -> ref[value] Int:
    return value

def main():
    var value = 1
    ref view = borrow(value)
    value = 2
    print(view)
