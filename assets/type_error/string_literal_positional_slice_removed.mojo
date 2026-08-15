# Positional slicing on a StringLiteral value was removed at the audited
# head along with nominal String positional slicing; the units are spelled
# through keyword slices.
# expect: positional slicing was removed
def main():
    print("hello"[1:4])
