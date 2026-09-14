# The receiver is a direct argument too: `s.rstrip().upper()` assigned back
# to `s` reads `s`'s owned bytes through `self`. The pinned Mojo rejects
# it with the same text.
# expect: aliasing values passed immutably to 'self' argument and constructed as a result in 'upper' call
def main():
    var s = String("abc  ")
    s = s.rstrip().upper()
    print(s)
