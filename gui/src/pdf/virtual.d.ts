declare module "virtual:world-pdf/resources" {
  const resources: Record<string, () => Promise<{ default: string }>>;
  export default resources;
}
