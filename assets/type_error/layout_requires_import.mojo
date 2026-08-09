# expect: Undefined variable 'Layout'
# The layout package is import-only, like upstream — never in the prelude.
def main():
    var l = Layout.row_major(2, 2)
    print(l)
