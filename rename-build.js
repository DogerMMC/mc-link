import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const pkg = JSON.parse(fs.readFileSync(path.join(__dirname, 'package.json'), 'utf8'));
const version = pkg.version;

const bundleDir = path.join(__dirname, 'src-tauri', 'target', 'release', 'bundle');

if (!fs.existsSync(bundleDir)) {
    console.log(`错误: 未找到构建目录 ${bundleDir}`);
    process.exit(1);
}

const renameFiles = (dir) => {
    const files = fs.readdirSync(dir);
    
    files.forEach(file => {
        const filePath = path.join(dir, file);
        const stat = fs.statSync(filePath);
        
        if (stat.isDirectory()) {
            renameFiles(filePath);
        } else {
            const ext = path.extname(file);
            const name = path.basename(file, ext);
            
            if (name.includes('mc-link') && !name.includes(`-${version}`)) {
                const newName = `${name}-${version}${ext}`;
                const newPath = path.join(dir, newName);
                
                if (fs.existsSync(newPath)) {
                    fs.unlinkSync(newPath);
                }
                
                fs.renameSync(filePath, newPath);
                console.log(`已重命名: ${file} -> ${newName}`);
            }
        }
    });
};

console.log(`开始重命名构建文件，版本号: ${version}`);
renameFiles(bundleDir);

const exePath = path.join(__dirname, 'src-tauri', 'target', 'release', 'mc-link.exe');
if (fs.existsSync(exePath)) {
    const newExePath = path.join(__dirname, 'src-tauri', 'target', 'release', `mc-link-${version}.exe`);
    if (fs.existsSync(newExePath)) {
        fs.unlinkSync(newExePath);
    }
    fs.copyFileSync(exePath, newExePath);
    console.log(`已复制: mc-link.exe -> mc-link-v${version}.exe`);
}

console.log('重命名完成!');