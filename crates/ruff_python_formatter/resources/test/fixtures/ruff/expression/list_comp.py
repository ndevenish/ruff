[i for i in []]

[i for i in [1,]]

[
    a  # a
    for # for
    c  # c
    in  # in
    e  # e
]

[
    # above a
    a  # a
    # above for
    for # for
    # above c
    c  # c
    # above in
    in  # in
    # above e
    e  # e
    # above if
    if  # if
    # above f
    f  # f
    # above if2
    if  # if2
    # above g
    g  # g
]

[
    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa + bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb + [dddddddddddddddddd, eeeeeeeeeeeeeeeeeee]
    for
    ccccccccccccccccccccccccccccccccccccccc,
    ddddddddddddddddddd, [eeeeeeeeeeeeeeeeeeeeee, fffffffffffffffffffffffff]
    in
    eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeffffffffffffffffffffffffggggggggggggggggggggghhhhhhhhhhhhhhothermoreeand_even_moreddddddddddddddddddddd
    if
    fffffffffffffffffffffffffffffffffffffffffff < gggggggggggggggggggggggggggggggggggggggggggggg < hhhhhhhhhhhhhhhhhhhhhhhhhh
    if
    gggggggggggggggggggggggggggggggggggggggggggg
]

# Regression tests for https://github.com/astral-sh/ruff/issues/5911
selected_choices = [
    str(v) for v in value if str(v) not in self.choices.field.empty_values
]

selected_choices = [
    str(v)
    for vvvvvvvvvvvvvvvvvvvvvvv in value if str(v) not in self.choices.field.empty_values
]

# Tuples with BinOp
[i for i in (aaaaaaaaaaaaaaaaaaaaaa + bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, ccccccccccccccccccccc)]
[(aaaaaaaaaaaaaaaaaaaaaa + bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, ccccccccccccccccccccc) for i in b]

a = [
    aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    for f in bbbbbbbbbbbbbbb
    if f not in ccccccccccc
]

a = [
    [1, 2, 3,]
    for f in bbbbbbbbbbbbbbb
    if f not in ccccccccccc
]

aaaaaaaaaaaaaaaaaaaaa = [
    o for o in self.registry.values if o.__class__ is not ModelAdmin
]
