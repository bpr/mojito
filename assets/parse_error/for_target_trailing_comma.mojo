# expect: unexpected token in expression
# A loop target list takes no trailing comma.
def main():
    for a, in [(7,), (8,)]:
        print(a)
