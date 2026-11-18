"""Builds the Rustic agent mascot and exports src/assets/mascot/mascot.glb.

Run headless:
  Blender -b --factory-startup --python scripts/mascot/build_mascot.py

The GLB carries geometry, pivots and mouth shape keys only. Fur, colours and
all motion are applied at runtime (src/components/agent/mascot3d/), so the
character can follow the UI theme. Blender is Z-up facing -Y; the exporter
converts to glTF Y-up facing +Z.
"""
import math
import os

import bmesh
import bpy
from mathutils import Matrix, Vector

OUT = os.path.normpath(os.path.join(
    os.path.dirname(os.path.abspath(__file__)), '..', '..', 'src', 'assets', 'mascot', 'mascot.glb'))

BODY_C = Vector((0.0, 0.0, 0.88))
RX, RY, RZ = 1.0, 0.9, 0.95
BASE_K = 0.16
PEAR = 0.12

CUBE_FACES = [
    (Vector((1, 0, 0)), Vector((0, 1, 0)), Vector((0, 0, 1))),
    (Vector((-1, 0, 0)), Vector((0, -1, 0)), Vector((0, 0, 1))),
    (Vector((0, 1, 0)), Vector((-1, 0, 0)), Vector((0, 0, 1))),
    (Vector((0, -1, 0)), Vector((1, 0, 0)), Vector((0, 0, 1))),
    (Vector((0, 0, 1)), Vector((1, 0, 0)), Vector((0, 1, 0))),
    (Vector((0, 0, -1)), Vector((1, 0, 0)), Vector((0, -1, 0))),
]


def smoothstep(a, b, x):
    """Hermite step between a and b (works with a > b for a falling edge)."""
    t = max(0.0, min(1.0, (x - a) / (b - a)))
    return t * t * (3 - 2 * t)


def body_point(d):
    """Maps a unit direction to the body surface: pear profile with a flattened base."""
    zz = d.z
    f = zz if zz >= 0 else zz + BASE_K * zz * zz
    pear = 1.0 - PEAR * zz
    return Vector((RX * pear * d.x, RY * pear * d.y, RZ * f))


def body_front_y(x, z):
    """Body-local y of the front surface at (x, z)."""
    t = z / RZ
    zz = t if t >= 0 else (-1 + math.sqrt(max(0.0, 1 + 4 * BASE_K * t))) / (2 * BASE_K)
    pear = 1.0 - PEAR * zz
    dx = x / (RX * pear)
    return -RY * pear * math.sqrt(max(0.0, 1 - dx * dx - zz * zz))


def body_front_normal(x, z):
    """Outward normal of the front surface at (x, z)."""
    e = 1e-3
    du = Vector((2 * e, body_front_y(x + e, z) - body_front_y(x - e, z), 0))
    dv = Vector((0, body_front_y(x, z + e) - body_front_y(x, z - e), 2 * e))
    n = dv.cross(du).normalized()
    return n if n.y < 0 else -n


def body_fur(p):
    """Per-vertex groom for the body: (fur length scale, ambient occlusion)."""
    face = smoothstep(0.85, 0.35, math.hypot(p.x / 1.0, (p.z + 0.1) / 0.72)) * smoothstep(0.0, -0.35, p.y)
    length = (1.0 - 0.55 * face) * (0.45 + 0.55 * smoothstep(-0.8, -0.5, p.z))
    ao = 0.6 + 0.4 * smoothstep(-0.8, 0.15, p.z)
    return length, ao


def new_object(name, mesh, parent=None, location=(0, 0, 0)):
    """Creates an object (or an empty when mesh is None) parented in parent space."""
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    obj.parent = parent
    obj.location = location
    return obj


def finish_mesh(bm, name, groom=None, smooth=True):
    """Converts a bmesh to mesh data, optionally baking the groom into a colour attribute."""
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    for f in bm.faces:
        f.smooth = smooth
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    if groom is not None:
        attr = mesh.color_attributes.new('Color', 'FLOAT_COLOR', 'POINT')
        for i, v in enumerate(mesh.vertices):
            length, ao = groom(v.co)
            attr.data[i].color = (length, ao, 0.0, 1.0)
        mesh.color_attributes.active_color = attr
    return mesh


def cube_sphere(name, n, shape, groom):
    """Quad sphere with near-uniform cells; UV u = cube face + s, v = t (drives fur strands)."""
    bm = bmesh.new()
    uv = bm.loops.layers.uv.new('UVMap')
    for fi, (ax, t1, t2) in enumerate(CUBE_FACES):
        grid = []
        for j in range(n + 1):
            row = []
            for i in range(n + 1):
                a = math.tan((i / n * 2 - 1) * math.pi / 4)
                b = math.tan((j / n * 2 - 1) * math.pi / 4)
                row.append(bm.verts.new(shape((ax + t1 * a + t2 * b).normalized())))
            grid.append(row)
        for j in range(n):
            for i in range(n):
                f = bm.faces.new((grid[j][i], grid[j][i + 1], grid[j + 1][i + 1], grid[j + 1][i]))
                for loop, (ii, jj) in zip(f.loops, ((i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1))):
                    loop[uv].uv = (fi + ii / n, jj / n)
    bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=1e-5)
    return finish_mesh(bm, name, groom)


def ellipsoid(radii, offset=(0, 0, 0), rot=None, floor=None):
    """Shape function for cube_sphere: an offset, optionally rotated and floored ellipsoid."""
    off = Vector(offset)

    def shape(d):
        p = Vector((d.x * radii[0], d.y * radii[1], d.z * radii[2])) + off
        if rot is not None:
            p = rot @ p
        if floor is not None and p.z < floor:
            p.z = floor
        return p
    return shape


def tube_coords(path, segs=28, ring=8, radius=0.03, taper=True, depth=None):
    """Vertex positions of a tube along path(s) in the XZ plane, s in [-1, 1]."""
    coords = []
    for k in range(segs + 1):
        s = k / segs * 2 - 1
        p = Vector(path(s))
        q = Vector(path(min(1.0, s + 1e-3))) - Vector(path(max(-1.0, s - 1e-3)))
        t = Vector((q.x, 0, q.z)).normalized() if q.length > 1e-9 else Vector((1, 0, 0))
        nrm = Vector((-t.z, 0, t.x))
        r = radius * (math.sqrt(max(0.0, 1 - abs(s) ** 10)) if taper else 1.0)
        for m in range(ring):
            a = m / ring * math.tau
            v = p + nrm * (math.cos(a) * r) + Vector((0, 1, 0)) * (math.sin(a) * r * 0.8)
            if depth is not None:
                v.y += depth(p.x, p.z)
            coords.append(v)
    return coords


def tube_bmesh(bm, coords, segs=28, ring=8):
    """Adds the faces for one tube_coords() vertex block to bm."""
    verts = [bm.verts.new(c) for c in coords]
    for k in range(segs):
        for m in range(ring):
            a = k * ring + m
            b = k * ring + (m + 1) % ring
            bm.faces.new((verts[a], verts[b], verts[b + ring], verts[a + ring]))
    return verts


def uv_sphere_mesh(name, radii, seg=28, rings=14):
    """Smooth ellipsoid for hard-surface parts (eyes, glints, tongue, logo)."""
    bm = bmesh.new()
    bmesh.ops.create_uvsphere(bm, u_segments=seg, v_segments=rings, radius=1.0)
    bmesh.ops.scale(bm, vec=Vector(radii), verts=bm.verts[:])
    return finish_mesh(bm, name)


def box_mesh(name, size, center, bevel, rot=None):
    """Bevelled box, flat shaded (laptop parts)."""
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bmesh.ops.scale(bm, vec=Vector(size), verts=bm.verts[:])
    bmesh.ops.bevel(bm, geom=bm.edges[:], offset=bevel, segments=3, affect='EDGES', profile=0.5)
    bmesh.ops.translate(bm, vec=Vector(center), verts=bm.verts[:])
    if rot is not None:
        bmesh.ops.transform(bm, matrix=rot, verts=bm.verts[:])
    return finish_mesh(bm, name, smooth=False)


def build():
    """Assembles the character hierarchy."""
    root = new_object('Root', None)

    body = new_object('Body', cube_sphere('Body', 28, body_point, body_fur), root, BODY_C)

    for side, sx in (('L', -1), ('R', 1)):
        # Geometry hangs straight down from the shoulder pivot; the rest splay is an
        # object rotation so the runtime can pose the arm around the same pivot.
        arm = cube_sphere('Arm' + side, 10, ellipsoid((0.19, 0.19, 0.33), (0, 0, -0.25)),
                          lambda p: (0.75, 0.9))
        arm_obj = new_object('Arm' + side, arm, body, (0.93 * sx, -0.22, -0.02))
        arm_obj.rotation_euler = (0, math.radians(-42 * sx), 0)

        foot = cube_sphere('Foot' + side, 8, ellipsoid((0.25, 0.32, 0.16), (0, -0.26, 0.16), floor=0.015),
                           lambda p: (0.4, 0.75 + 0.25 * smoothstep(0.0, 0.2, p.z)))
        new_object('Foot' + side, foot, root, (0.40 * sx, 0.02, 0.0))

    face = new_object('Face', None, body)
    eye_x, eye_z = 0.36, -0.02
    for side, sx in (('L', -1), ('R', 1)):
        x = eye_x * sx
        n = body_front_normal(x, eye_z)
        pos = Vector((x, body_front_y(x, eye_z), eye_z)) + n * 0.03
        yaw = math.atan2(-n.x, -n.y)

        eye = new_object('Eye' + side, uv_sphere_mesh('Eye', (0.155, 0.08, 0.21)), face, pos)
        eye.rotation_euler = (0, 0, -yaw)
        new_object('Pupil' + side, uv_sphere_mesh('Pupil', (0.095, 0.028, 0.13)), eye, (0, -0.064, 0))
        new_object('Glint' + side, uv_sphere_mesh('Glint', (0.042, 0.02, 0.042), 16, 8), eye, (-0.05, -0.074, 0.075))
        new_object('GlintSmall' + side, uv_sphere_mesh('GlintSmall', (0.021, 0.012, 0.021), 12, 6),
                   eye, (0.045, -0.076, -0.07))

        bm = bmesh.new()
        tube_bmesh(bm, tube_coords(lambda s: (0.15 * math.sin(s * 1.2), 0, 0.15 * math.cos(s * 1.2) - 0.09),
                                   radius=0.042))
        happy = new_object('EyeHappy' + side, finish_mesh(bm, 'EyeHappy'), face, pos + n * 0.04)
        happy.rotation_euler = (0, 0, -yaw)

        bm = bmesh.new()
        tube_bmesh(bm, tube_coords(lambda s: (0.13 * s, 0, 0.13 * s), radius=0.04))
        tube_bmesh(bm, tube_coords(lambda s: (0.13 * s, 0, -0.13 * s), radius=0.04))
        cross = new_object('EyeCross' + side, finish_mesh(bm, 'EyeCross'), face, pos + n * 0.04)
        cross.rotation_euler = (0, 0, -yaw)

    mouth_z = -0.27
    mouth_y = body_front_y(0, mouth_z)

    def lift(x, z):
        return body_front_y(x, mouth_z + z) - mouth_y - 0.085

    shapes = {
        'Basis': dict(path=lambda s: (0.085 * s, 0, -0.03 * (1 - s * s) + 0.01)),
        'Smile': dict(path=lambda s: (0.15 * s, 0, -0.10 * (1 - s * s) + 0.035)),
        'Flat': dict(path=lambda s: (0.075 * s, 0, 0.0)),
        'Wavy': dict(path=lambda s: (0.16 * s, 0, 0.032 * math.sin(s * math.pi * 1.5))),
        'O': dict(path=lambda s: (0.06 * math.sin(s * math.pi), 0, -0.07 * math.cos(s * math.pi) - 0.01),
                  taper=False),
    }
    bm = bmesh.new()
    tube_bmesh(bm, tube_coords(radius=0.03, depth=lift, **shapes['Basis']))
    mouth = new_object('Mouth', finish_mesh(bm, 'Mouth'), face, (0, mouth_y, mouth_z))
    for key, spec in shapes.items():
        sk = mouth.shape_key_add(name=key, from_mix=False)
        if key == 'Basis':
            continue
        # New keys default to value 1.0 and the exporter bakes that into the mesh weights.
        sk.value = 0.0
        for i, co in enumerate(tube_coords(radius=0.03, depth=lift, **spec)):
            sk.data[i].co = co
    new_object('Tongue', uv_sphere_mesh('Tongue', (0.05, 0.025, 0.045), 16, 8), mouth,
               (0.03, lift(0.03, -0.05) - 0.005, -0.05))

    laptop = new_object('Laptop', None, root, (0, -1.24, 0))
    new_object('LaptopBase', box_mesh('LaptopBase', (1.24, 0.5, 0.05), (0, 0.25, 0.025), 0.02), laptop)
    tilt = Matrix.Rotation(math.radians(14), 4, 'X')
    new_object('LaptopLid', box_mesh('LaptopLid', (1.24, 0.045, 0.66), (0, 0, 0.33), 0.02, tilt),
               laptop, (0, 0, 0.05))
    logo = new_object('LaptopLogo', uv_sphere_mesh('LaptopLogo', (0.075, 0.012, 0.075), 20, 8), laptop,
                      Vector((0, 0, 0.05)) + tilt @ Vector((0, -0.028, 0.36)))
    logo.rotation_euler = (math.radians(14), 0, 0)


def main():
    """Builds the scene from empty and exports the GLB."""
    bpy.ops.wm.read_factory_settings(use_empty=True)
    build()
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=OUT,
        export_format='GLB',
        export_yup=True,
        export_apply=False,
        export_materials='NONE',
        export_vertex_color='ACTIVE',
        export_active_vertex_color_when_no_material=True,
        export_all_vertex_colors=False,
        export_normals=True,
        export_texcoords=True,
        export_morph=True,
        export_morph_normal=False,
        export_animations=False,
        export_skins=False,
    )
    print('MASCOT_EXPORTED', OUT, os.path.getsize(OUT))


main()
