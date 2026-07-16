# Argument conventions on ordinary parameters: `mut` (a reference whose
# mutations are written back) and `var` (takes ownership).
def update(mut total: Int, var label: String):
    total = total + 1
