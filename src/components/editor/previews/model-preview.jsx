import React, { useEffect, useRef, useState } from 'react';
import { useFileReloadVersion } from '@/lib/use-file-change';
import { fetchFileBytes } from '@/lib/file-bytes';
import { basename } from '@/state/editor';
import { Skeleton } from '@/components/ui/skeleton';

/** Reads a CSS custom property (theme tokens are oklch) as a three.js colour string. */
function themeColor(token, fallback) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(token).trim();
  if (!v) return fallback;
  const probe = document.createElement('canvas').getContext('2d');
  probe.fillStyle = fallback;
  probe.fillStyle = v;
  return probe.fillStyle;
}

/** 3D model viewer for GLB/glTF/STL with orbit controls (three.js, lazy-loaded). */
export default function ModelPreview({ tab }) {
  const mountRef = useRef(null);
  const [info, setInfo] = useState(null);
  const [error, setError] = useState(null);
  const reloadVersion = useFileReloadVersion(tab.path);

  useEffect(() => {
    let disposed = false;
    let cleanup = () => {};
    setInfo(null);
    setError(null);
    (async () => {
      try {
        const ext = tab.path.toLowerCase().split('.').pop();
        const [THREE, { OrbitControls }, loaders, bytes] = await Promise.all([
          import('three'),
          import('three/examples/jsm/controls/OrbitControls.js'),
          ext === 'stl'
            ? import('three/examples/jsm/loaders/STLLoader.js')
            : import('three/examples/jsm/loaders/GLTFLoader.js'),
          fetchFileBytes(tab.path, { version: reloadVersion }),
        ]);
        if (disposed) return;
        const mount = mountRef.current;
        const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
        renderer.setPixelRatio(window.devicePixelRatio || 1);
        renderer.outputColorSpace = THREE.SRGBColorSpace;
        mount.appendChild(renderer.domElement);
        const scene = new THREE.Scene();
        scene.add(new THREE.HemisphereLight(0xffffff, 0x444444, 1.6));
        const sun = new THREE.DirectionalLight(0xffffff, 2);
        sun.position.set(3, 5, 4);
        scene.add(sun);
        const camera = new THREE.PerspectiveCamera(45, 1, 0.01, 1e6);
        const controls = new OrbitControls(camera, renderer.domElement);
        controls.enableDamping = true;

        let object;
        let triangles = 0;
        if (ext === 'stl') {
          const geom = new loaders.STLLoader().parse(bytes.buffer);
          geom.computeVertexNormals();
          const mat = new THREE.MeshStandardMaterial({
            color: themeColor('--primary', '#8899aa'),
            metalness: 0.1,
            roughness: 0.6,
          });
          object = new THREE.Mesh(geom, mat);
          triangles = geom.index ? geom.index.count / 3 : geom.attributes.position.count / 3;
        } else {
          // Embedded (.glb / data-URI .gltf) models load fully; external
          // buffers/textures beside a .gltf aren't resolved.
          const gltf = await new Promise((resolve, reject) =>
            new loaders.GLTFLoader().parse(bytes.buffer, '', resolve, reject),
          );
          object = gltf.scene;
          object.traverse((o) => {
            if (o.isMesh && o.geometry) {
              const g = o.geometry;
              triangles += g.index ? g.index.count / 3 : (g.attributes.position?.count || 0) / 3;
            }
          });
        }
        scene.add(object);

        const box = new THREE.Box3().setFromObject(object);
        const size = box.getSize(new THREE.Vector3());
        const center = box.getCenter(new THREE.Vector3());
        const radius = Math.max(size.x, size.y, size.z) || 1;
        object.position.sub(center);
        camera.position.set(radius * 1.2, radius * 0.9, radius * 1.6);
        camera.near = radius / 1000;
        camera.far = radius * 1000;
        controls.target.set(0, 0, 0);
        controls.update();
        setInfo(`${Math.round(triangles).toLocaleString()} triangles · ${size.x.toFixed(2)} × ${size.y.toFixed(2)} × ${size.z.toFixed(2)}`);

        const resize = () => {
          const w = mount.clientWidth || 1;
          const h = mount.clientHeight || 1;
          renderer.setSize(w, h);
          camera.aspect = w / h;
          camera.updateProjectionMatrix();
        };
        resize();
        const ro = new ResizeObserver(resize);
        ro.observe(mount);
        let raf = 0;
        const loop = () => {
          controls.update();
          renderer.render(scene, camera);
          raf = requestAnimationFrame(loop);
        };
        loop();
        cleanup = () => {
          cancelAnimationFrame(raf);
          ro.disconnect();
          controls.dispose();
          scene.traverse((o) => {
            o.geometry?.dispose?.();
            const mats = Array.isArray(o.material) ? o.material : o.material ? [o.material] : [];
            mats.forEach((m) => {
              Object.values(m).forEach((v) => v?.isTexture && v.dispose());
              m.dispose();
            });
          });
          renderer.dispose();
          renderer.domElement.remove();
        };
      } catch (e) {
        if (!disposed) setError(`Could not load this model (${String(e?.message || e)}).`);
      }
    })();
    return () => {
      disposed = true;
      cleanup();
    };
  }, [tab.path, reloadVersion]);

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-muted/20 px-2 text-xs text-muted-foreground">
        <span className="truncate">{basename(tab.path)}</span>
        {info && <span className="ml-auto truncate text-[11px]">{info} · drag to orbit, scroll to zoom</span>}
      </div>
      <div ref={mountRef} className="preview-checkerboard relative min-h-0 flex-1">
        {!info && !error && <Skeleton className="absolute inset-8" />}
        {error && <div className="flex h-full items-center justify-center p-6 text-sm text-destructive">{error}</div>}
      </div>
    </div>
  );
}
