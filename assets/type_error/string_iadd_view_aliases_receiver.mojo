# expect: aliasing values passed mutably to 'self' argument and passed immutably to 'other' argument in '__iadd__' call
# Appending a view of the string being appended to borrows the receiver's
# own bytes while `__iadd__` mutates it, as upstream rejects.
def main():
    var s = String("abc  ")
    s += s.rstrip()
    print(s)
