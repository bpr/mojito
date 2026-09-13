# expect: var count
# A var-less introduction (`count = 10` on an undeclared name) is rejected:
# Mojito requires `var` to declare a new variable. The pinned Mojo only
# deprecates the spelling — it warns and runs — so this is Mojito holding the
# stricter line on a form upstream is retiring.
def main():
    count = 10
    print(count)
